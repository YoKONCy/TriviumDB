"""独立统计与编码参照；不调用C++评分实现。"""
from pathlib import Path
import hashlib,json,os
import numpy as np

HERE=Path(__file__).resolve().parent
ROOT=Path(os.environ.get('QUIVER_EVALUATION_DATA_ROOT', 'evaluation-data'))
RUN=ROOT/'stage-c-results/quiver-reviewer-mechanism-200k-20261018-v1'
def read(p):return json.loads(Path(p).read_text(encoding='utf-8'))
def digest(p):
    h=hashlib.sha256()
    with Path(p).open('rb') as f:
        for block in iter(lambda:f.read(8*1024**2),b''):h.update(block)
    return h.hexdigest()
def write(p,value):
    p=Path(p);p.parent.mkdir(parents=True,exist_ok=True)
    temp=p.with_name(p.name+'.tmp');temp.write_text(json.dumps(value,ensure_ascii=False,indent=2,allow_nan=False),encoding='utf-8');os.replace(temp,p)
def immutable(p,value):
    p=Path(p)
    if p.exists():assert read(p)==value,'冻结记录变化：'+str(p)
    else:write(p,value)
def encode(x,mode,center,tau):
    y=np.asarray(x,dtype=np.float32)-(center if mode=='centered' else np.float32(0))
    threshold=np.cumsum(np.abs(y),axis=1,dtype=np.float32)[:,-1:]/np.float32(y.shape[1])
    strong=np.abs(y)>(tau if mode=='perdim' else threshold)
    counts=strong.sum(axis=1)
    weak_count=y.shape[1]-counts
    a=np.divide(np.where(strong,0,np.abs(y)).sum(axis=1,dtype=np.float64),weak_count,out=np.zeros(len(y)),where=weak_count>0).astype(np.float32)
    b=np.divide(np.where(strong,np.abs(y),0).sum(axis=1,dtype=np.float64),counts,out=np.zeros(len(y)),where=counts>0).astype(np.float32)
    delta=(b-a).astype(np.float32)
    norm=(weak_count.astype(np.float32)*a*a+counts.astype(np.float32)*b*b).astype(np.float32)
    reconstruction=(np.where(strong,b[:,None],a[:,None])*np.where(y>0,np.float32(1),np.float32(-1))).astype(np.float32)
    return dict(pos=y>0,strong=strong,weak=a,delta=delta,norm=norm,count=counts,reconstruction=reconstruction)
def bits_reference(encoded):
    dim=encoded['pos'].shape[1];pad=(-dim)%64
    pos=np.packbits(np.pad(encoded['pos'],((0,0),(0,pad))),axis=1,bitorder='little').copy().view('<u8')
    strong=np.packbits(np.pad(encoded['strong'],((0,0),(0,pad))),axis=1,bitorder='little').copy().view('<u8')
    return np.concatenate([pos,strong],axis=1)
def summary(values):
    v=np.asarray(values,dtype=np.float64);v=v[np.isfinite(v)]
    if not len(v):return None
    return dict(n=len(v),mean=float(v.mean()),std=float(v.std()),p05=float(np.quantile(v,.05)),median=float(np.median(v)),p95=float(np.quantile(v,.95)))
def interval(values):
    v=np.asarray(values,dtype=np.float64)
    if not len(v):return None
    rng=np.random.default_rng(20261018);boots=[]
    for _ in range(2000):boots.append(v[rng.integers(len(v),size=len(v))].mean())
    return [float(x) for x in np.quantile(boots,[.025,.975])]
def graph(path,n):
    v=np.fromfile(path,dtype='<u4');assert v[0]==n
    rows=[];p=1
    for u in range(n):
        k=int(v[p]);p+=1;row=v[p:p+k];p+=k
        assert k<=64 and len(row)==len(np.unique(row)) and (row<n).all() and u not in row
        rows.append(row)
    assert p==len(v)
    return rows
def exact_distances(q,x,cosine):
    q=np.asarray(q,dtype=np.float64);x=np.asarray(x,dtype=np.float64)
    if cosine:
        return 1-np.einsum('...d,...d->...',q,x)/np.maximum(np.linalg.norm(q,axis=-1)*np.linalg.norm(x,axis=-1),1e-300)
    return np.square(q-x).sum(axis=-1)
