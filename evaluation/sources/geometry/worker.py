"""四库200K的输入、精确真值、几何统计和独立质量复核。"""
from common import *
import math,shutil,sys,time,zipfile
from scipy.spatial.distance import cdist
from threadpoolctl import threadpool_limits

PLAN=read(HERE/'plan.json')
def prepared(job,work):
    dim,n=job['dim'],job['n']
    inputs=job['inputs']
    for name in ['train','test']:
        item=inputs[name];p=Path(item['path'])
        assert p.stat().st_size==item['bytes'] and digest(p)==item['sha256'],'源输入身份不符：'+str(p)
    x=np.memmap(inputs['train']['path'],dtype='<f4',mode='r').reshape(-1,dim)[:n]
    q=np.memmap(inputs['test']['path'],dtype='<f4',mode='r').reshape(-1,dim)[job['query_ids']]
    assert x.shape==(n,dim) and q.shape==(744,dim) and np.isfinite(x).all() and np.isfinite(q).all()
    x.tofile(work/'base.f32');q.tofile(work/'queries.f32')
    center=x.sum(axis=0,dtype=np.float64)/n
    tau=np.abs(x).sum(axis=0,dtype=np.float64)/n
    center.astype('<f4').tofile(work/'centroid.f32');tau.astype('<f4').tofile(work/'dim-threshold.f32')
    cosine=job['metric']=='cosine';np.array([int(cosine)],dtype='<u4').tofile(work/'metric.u32')
    write(work/'query-ids.json',job['query_ids'])
    nq=len(q);best=np.full((nq,32),np.inf);ids=np.full((nq,32),n,dtype=np.uint32)
    # 每块按距离及原编号稳定排序；SIFT并列不使用不稳定分区挑真值。
    with threadpool_limits(limits=8):
        for first in range(0,n,8192):
            block=np.asarray(x[first:first+8192],dtype=np.float64);qq=q.astype(np.float64)
            values=cdist(qq,block,metric='cosine' if cosine else 'sqeuclidean')
            order=np.argsort(values,axis=1,kind='stable')[:,:32]
            new=np.take_along_axis(values,order,axis=1)
            pool=np.concatenate([best,new],axis=1);pids=np.concatenate([ids,(first+order).astype(np.uint32)],axis=1)
            choose=np.argsort(pool,axis=1,kind='stable')[:,:32]
            best=np.take_along_axis(pool,choose,axis=1);ids=np.take_along_axis(pids,choose,axis=1)
            write(work/'prepare-progress.json',dict(state='计算200K完整FP64真值',rows=min(n,first+8192),total=n))
    ids.astype('<u4').tofile(work/'truth32.u32');best.astype('<f8').tofile(work/'truth32-distance.f64')
    # 独立以点积/范数表达式抽查完整200K，而非复用cdist构造器。
    checks=[]
    xx=np.asarray(x,dtype=np.float64);xn=np.einsum('ij,ij->i',xx,xx)
    for qi in [0,173,371,559,743]:
        query=q[qi].astype(np.float64)
        if cosine:
            v=1-np.einsum('ij,j->i',xx,query)/np.sqrt(xn*np.dot(query,query))
        else:v=np.einsum('ij,ij->i',xx-query,xx-query)
        selected=np.lexsort((np.arange(n),v))[:32]
        assert np.allclose(v[ids[qi]],best[qi],rtol=1e-10,atol=1e-10)
        assert np.max(v[ids[qi,:10]])<=np.sort(v)[9]+1e-10
        checks.append(dict(query=qi,top10_set_equal=bool(set(selected[:10])==set(ids[qi,:10])),max_error=float(np.max(np.abs(v[ids[qi]]-best[qi])))))
    identities={p.name:dict(bytes=p.stat().st_size,sha256=digest(p)) for p in work.iterdir() if p.is_file() and p.name!='prepare-progress.json'}
    write(work/'input-manifest.json',dict(status='passed',dataset=job,prepared=identities,truth_checks=checks,n=n,queries=nq,metric=job['metric'],normalization_changed=False))
    write(work/'result.json',dict(status='passed',n=n,queries=nq,manifest_sha256=digest(work/'input-manifest.json')))

def audit_gate(data,out,dim,n):
    x=np.memmap(data/'base.f32',dtype='<f4',mode='r').reshape(n,dim)
    center=np.fromfile(data/'centroid.f32',dtype='<f4');tau=np.fromfile(data/'dim-threshold.f32',dtype='<f4')
    pairs=np.fromfile(out/'kernel-pairs.f64',dtype='<f8').reshape(3,256,6)
    for number,mode in enumerate(['standard','centered','perdim']):
        e=encode(x[:512],mode,center,tau)
        actual=np.fromfile(out/(mode+'-gate-bits.u64'),dtype='<u8').reshape(len(e['pos']),-1)
        assert np.array_equal(actual,bits_reference(e)),'C++位码与独立Python编码不同'
        meta=np.fromfile(out/(mode+'-gate-meta.f32'),dtype='<f4').reshape(-1,4)
        reference=np.stack([e['weak'],e['delta'],e['norm'],e['count']],axis=1)
        assert np.allclose(meta,reference,rtol=3e-6,atol=1e-6)
        a=pairs[number,:,0].astype(int);b=pairs[number,:,1].astype(int)
        ea=encode(x[a],mode,center,tau);eb=encode(x[b],mode,center,tau)
        sq=np.square(ea['reconstruction'].astype(np.float64)-eb['reconstruction']).sum(axis=1)
        fixeda=np.where(ea['strong'],2,1)*np.where(ea['pos'],1,-1)
        fixedb=np.where(eb['strong'],2,1)*np.where(eb['pos'],1,-1)
        assert np.allclose(sq,pairs[number,:,2],rtol=3e-5,atol=1e-4)
        assert np.array_equal(4*dim-(fixeda*fixedb).sum(axis=1),pairs[number,:,4])
    write(out/'independent-gate.json',dict(status='passed',bits_checked=512,variants=3,pairs=768))
    write(out/'result.json',dict(status='passed',gate_sha256=digest(out/'independent-gate.json')))

def geometry(job,data,work):
    dim,n=job['dim'],job['n'];x=np.memmap(data/'base.f32',dtype='<f4',mode='r').reshape(n,dim)
    q=np.fromfile(data/'queries.f32',dtype='<f4').reshape(-1,dim);gt=np.fromfile(data/'truth32.u32',dtype='<u4').reshape(len(q),32)
    center=np.fromfile(data/'centroid.f32',dtype='<f4');tau=np.fromfile(data/'dim-threshold.f32',dtype='<f4');rng=np.random.default_rng(20261018)
    aa=rng.integers(n,size=16384);bb=rng.integers(n,size=16384);bb=np.where(bb==aa,(bb+1)%n,bb)
    cosine=lambda a,b:np.einsum('...d,...d->...',a.astype(np.float64),b.astype(np.float64))/np.maximum(np.linalg.norm(a.astype(np.float64),axis=-1)*np.linalg.norm(b.astype(np.float64),axis=-1),1e-300)
    random_angles=np.degrees(np.arccos(np.clip(cosine(x[aa],x[bb]),-1,1)))
    near_angles=np.degrees(np.arccos(np.clip(cosine(q[:,None,:],x[gt[:,:10]]),-1,1)))
    farids=rng.integers(n,size=(len(q),64));candidate_ids=np.concatenate([gt,farids],axis=1)
    # 只形成预定候选样本，不运行全库近似排序；候选集合对所有编码相同。
    true=exact_distances(q[:,None,:],x[candidate_ids],job['metric']=='cosine')
    order=np.argsort(true,axis=1,kind='stable');left=order[:,:-1];right=order[:,1:]
    per_variant={};raw=dict(random_pair_a=aa,random_pair_b=bb,random_angles=random_angles,near_angles=near_angles,candidate_ids=candidate_ids,true_distances=true)
    sample_ids=rng.integers(n,size=4096)
    for mode in ['standard','centered','perdim']:
        eq=encode(q,mode,center,tau);approx=np.empty(candidate_ids.shape)
        for qi in range(len(q)):
            eb=encode(x[candidate_ids[qi]],mode,center,tau)
            approx[qi]=np.square(eq['reconstruction'][qi].astype(np.float64)-eb['reconstruction']).sum(axis=1)
        lo=np.take_along_axis(approx,left,axis=1);hi=np.take_along_axis(approx,right,axis=1)
        strict=(np.take_along_axis(true,right,axis=1)-np.take_along_axis(true,left,axis=1))>1e-12
        inverted=(lo>hi)&strict;tied=(lo==hi)&strict
        error=inverted.sum(axis=1)/np.maximum(strict.sum(axis=1),1)
        es=encode(x[sample_ids],mode,center,tau);recon=es['reconstruction'].astype(np.float64)+(center if mode=='centered' else 0)
        mse=np.square(x[sample_ids].astype(np.float64)-recon).sum(axis=1);energy=np.square(x[sample_ids].astype(np.float64)).sum(axis=1)
        per_variant[mode]=dict(adjacent_rank_inversion=summary(error),adjacent_rank_inversion_query_ci=interval(error),ties=int(tied.sum()),strict_pairs=int(strict.sum()),
            reconstruction_mse_per_vector=summary(mse),empirical_sqnr_db=10*math.log10(float(energy.sum()/max(mse.sum(),1e-300))),
            sqnr_note='按本次实际重建值和原始向量能量计算；不是旧稿高斯量化保证',strong_fraction=summary(es['strong'].mean(axis=1)),corpus_metadata_bytes=dim*4 if mode!='standard' else 0)
        raw[mode+'_distances']=approx;raw[mode+'_inversion_per_query']=error
    truthdist=np.fromfile(data/'truth32-distance.f64',dtype='<f8').reshape(len(q),32)
    np.savez_compressed(work/'geometry-raw.npz',**raw)
    result=dict(status='passed',dataset=job['id'],random_pair_angles=summary(random_angles),near_pair_angles=summary(near_angles.reshape(-1)),
        near_query_mean_angle_ci=interval(near_angles.mean(axis=1)),exact_neighbor_gap_10_11=summary(truthdist[:,10]-truthdist[:,9]),variants=per_variant,
        interpretation='跨数据集相关性和局部诊断，不把角度单一统计量当因果保证；SIFT真值始终为原生L2')
    write(work/'result.json',result)

def audit_query(job,data,work):
    result=read(work/'query.json');nq=result['queries'];ef=result['ef'];dim=job['dim'];n=job['n']
    gt=np.fromfile(data/'truth32.u32',dtype='<u4').reshape(nq,32)
    labels=np.fromfile(work/'labels.u32',dtype='<u4').reshape(nq,10)
    pool=np.fromfile(work/'pool.u32',dtype='<u4').reshape(nq,ef)
    rerank=np.fromfile(work/'rerank.u32',dtype='<u4').reshape(nq,64)
    stats=np.fromfile(work/'stats.f64',dtype='<f8').reshape(nq,14)
    assert (labels<n).all() and all(len(np.unique(row))==10 for row in labels)
    hits=np.any(labels[:,:,None]==gt[:,:10,None].transpose(0,2,1),axis=2).sum(axis=1)
    assert np.array_equal(hits,stats[:,7]) and np.array_equal(stats[:,6],stats[:,7])
    assert np.all(stats[:,4]>=stats[:,5]) and np.all(stats[:,5]>=stats[:,6]) and np.all(stats[:,3]==64)
    x=np.memmap(data/'base.f32',dtype='<f4',mode='r').reshape(n,dim);q=np.fromfile(data/'queries.f32',dtype='<f4').reshape(nq,dim)
    for qi in [0,173,371,559,743]:
        exact=exact_distances(q[qi],x[rerank[qi]],job['metric']=='cosine');order=np.lexsort((rerank[qi],exact))[:10]
        assert set(rerank[qi,order])==set(labels[qi]),'末尾FP64精排独立复核不符'
    trace=np.fromfile(work/'trace.f64',dtype='<f8').reshape(-1,8)
    if len(trace):
        take=trace[np.linspace(0,len(trace)-1,min(len(trace),512),dtype=int)]
        qi=take[:,0].astype(int);u=take[:,1].astype(int);v=take[:,2].astype(int)
        assert np.allclose(exact_distances(q[qi],x[u],job['metric']=='cosine'),take[:,5],rtol=1e-9,atol=1e-9)
        assert np.allclose(exact_distances(q[qi],x[v],job['metric']=='cosine'),take[:,6],rtol=1e-9,atol=1e-9)
    write(work/'result.json',dict(status='passed',dataset=job['id'],nav=result['nav'],ef=ef,recall_at_10=float(hits.mean()/10),recall_query_ci=interval(hits/10),
        visited_oracle=float(stats[:,4].mean()/10),pool_oracle=float(stats[:,5].mean()/10),rerank_oracle=float(stats[:,6].mean()/10),
        expanded_mean=float(stats[:,1].mean()),scores_mean=float(stats[:,2].mean()),fp32_rerank_mean=64,
        sampled_progress_steps=int(stats[:,8].sum()),sampled_missed_progress_steps=int(stats[:,9].sum()),
        sampled_improving_edges=int(stats[:,10].sum()),sampled_accepted_improving_edges=int(stats[:,11].sum()),
        trace_records=len(trace),trace_note='前16条冻结查询在ef128下累计统计全部改善边；原始逐边记录每查询最多4096条，按原顺序截断，不能当全量无偏样本',
        raw_sha256={p.name:digest(p) for p in work.iterdir() if p.is_file() and p.name!='result.json'}))

def audit_build(job,data,work):
    n,d=job['n'],job['dim'];rows=graph(work/'graph.u32',n)
    events=np.fromfile(work/'prune-events.f64',dtype='<f8').reshape(-1,10)
    assert len(events)>0
    sample=events[np.linspace(0,len(events)-1,min(2048,len(events)),dtype=int)]
    x=np.memmap(data/'base.f32',dtype='<f4',mode='r').reshape(n,d)
    u,v,w=[sample[:,i].astype(int) for i in range(3)]
    exactuv=exact_distances(x[u],x[v],job['metric']=='cosine');exactvw=exact_distances(x[v],x[w],job['metric']=='cosine')
    assert np.allclose(exactuv,sample[:,6],rtol=1e-9,atol=1e-9) and np.allclose(exactvw,sample[:,7],rtol=1e-9,atol=1e-9)
    decided=(sample[:,3].astype(np.float32)*sample[:,5].astype(np.float32)<=sample[:,4].astype(np.float32))
    assert np.array_equal(decided,sample[:,8].astype(bool))
    kind=read(work/'build.json')['mode']
    if kind!='fp32':
        center=np.fromfile(data/'centroid.f32',dtype='<f4');tau=np.fromfile(data/'dim-threshold.f32',dtype='<f4')
        enc=[encode(x[a],kind,center,tau) for a in [u,v,w]]
        fixed=[np.where(e['strong'],2,1)*np.where(e['pos'],1,-1) for e in enc]
        assert np.array_equal(4*d-(fixed[0]*fixed[1]).sum(axis=1),sample[:,4])
        assert np.array_equal(4*d-(fixed[1]*fixed[2]).sum(axis=1),sample[:,5])
    disagreement=(events[:,3]*events[:,7]<=events[:,6])!=events[:,8].astype(bool)
    result=read(work/'build.json')|dict(pruning_disagreement=float(disagreement.mean()),pruning_events=len(events),independent_sampled_events=len(sample),
        pruning_note='实际前向剪枝中的固定源节点采样；每次剪枝前32次比较。FP64原始距离为对照；不是全图所有剪枝决策的无偏估计',
        graph_sha256=digest(work/'graph.u32'),pruning_raw_sha256=digest(work/'prune-events.f64'))
    write(work/'result.json',result)

def pack(job,folder,work):
    expected=6+len(PLAN['query_conditions'])*len(PLAN['query_efs'])
    receipts=list(folder.glob('*/receipt.json'))
    assert len(receipts)==expected+1,'数据、门禁、几何、四张图及三十查询检查点不完整'
    points=[read(folder/f'query-{g}-{nav}-ef{ef}'/'result.json')|dict(graph=g) for g,nav in PLAN['query_conditions'] for ef in PLAN['query_efs']]
    write(work/'result.json',dict(status='passed',dataset=job['id'],positions=len(points),points=points,geometry=read(folder/'geometry/result.json')))
    archive=folder/'evidence.zip'
    with zipfile.ZipFile(archive,'x',zipfile.ZIP_DEFLATED,compresslevel=1) as z:
        for p in folder.rglob('*'):
            if p.is_file() and p!=archive and p.suffix!='.tmp':z.write(p,str(p.relative_to(folder)))
        z.write(HERE/'plan.json','frozen-plan.json')
    write(folder/'archive-receipt.json',dict(status='passed',dataset=job['id'],archive_sha256=digest(archive),bytes=archive.stat().st_size))

if __name__=='__main__':
    action,identifier,workpath=sys.argv[1:4];job=next(j for j in PLAN['datasets'] if j['id']==identifier);folder=RUN/identifier;work=Path(workpath)
    data=folder/'data'
    with threadpool_limits(limits=8):
        if action=='prepare':prepared(job,work)
        elif action=='gate':audit_gate(data,work,job['dim'],job['n'])
        elif action=='geometry':geometry(job,data,work)
        elif action=='audit-query':audit_query(job,data,work)
        elif action=='audit-build':audit_build(job,data,work)
        elif action=='pack':pack(job,folder,work)
        else:raise ValueError('未知补证步骤')
