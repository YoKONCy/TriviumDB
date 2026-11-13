"""由冻结统计快照生成论文表格、矢量图与可追溯的数据清单。"""
import csv
import json
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding='utf-8')

from draw_paper_figures import METHODS, render_all

ROOT = Path(__file__).resolve().parent
DATA = json.loads((ROOT.parent / 'results/benchmark-snapshot.json').read_text(encoding='utf-8-sig'))
CONFIG = {c['id']: c for c in DATA['configs']}
CURVES = DATA['curves']
DATASETS = {d['id']: d for d in DATA['datasets']}
CORE = ['D03', 'D05', 'D14']
NAMES = {'D03': 'Cohere-1M', 'D05': 'DBpedia-1536', 'D14': 'Cohere-5M'}
for folder in ('figures', 'tables', 'evidence'):
    (ROOT / folder).mkdir(exist_ok=True)


def tex(value):
    return str(value).replace('&', r'\&').replace('_', r'\_').replace('%', r'\%')


def number(value, decimals=0):
    return r'\missing' if value is None else f'{value:,.{decimals}f}'


def target_curve(dataset, method, target):
    matches = [c for c in CURVES if c['dataset'] == dataset and c['method'] == method
        and any(t['target'] == target for t in c.get('targets', []))
        and '1' in c['points'] and c['points']['1'].get('qps') is not None
        and c['points']['1'].get('eligible', True)]
    if len(matches) != 1:
        raise ValueError(f'冻结目标位置不唯一：{dataset} / {method} / {target}，数量={len(matches)}')
    return matches[0]


def preparation(config):
    value = config['build']
    return value + (config.get('install') or 0) if value is not None else None


# 主表沿用调参集冻结选择，不按评估召回重新挑点。
rows = []
provenance = []
for ds in CORE:
    for method, (label, _, _) in METHODS.items():
        c = target_curve(ds, method, .95)
        p = c['points']['1']
        cfg = CONFIG[c['config']]
        values = [NAMES[ds], label, number(100 * p['recall'], 2)]
        values += [number(c['points'].get(str(t), {}).get('qps')) for t in (1, 8, 16, 32)]
        # 内存列给出导航组件与逻辑组件合计；独立FP32库可由两列之差得到。
        values += [number(preparation(cfg), 1), number(cfg['hot'], 3),
                   number(cfg['hot'] + cfg['cold'], 3), number(p['p99'], 3)]
        line = ' & '.join(tex(x) if not x.startswith('\\') else x for x in values) + r' \\'
        if method == 'QuIVer':
            line = line.replace('QuIVer', r'\textbf{QuIVer}')
        rows.append(line)
        provenance.append({'dataset': ds, 'method': method, 'curve': c['id'],
            'config': c['config'], 'parameters': c['parameters'], 'target': .95,
            'threads': list(c['points']), 'evidence': p['evidence']})
    rows.append(r'\addlinespace[3pt]')
(ROOT / 'tables/main-comparison.tex').write_text('\n'.join(rows), encoding='utf-8')

rows = []
for method in ('QuIVer', 'SymphonyQG', 'NGT-QG', 'Lucene BBQ + FP32', 'SVS LVQ8', 'HNSWlib FP32'):
    values = [METHODS[method][0]]
    for ds in CORE:
        c = target_curve(ds, method, .98)
        p = c['points']['1']
        values += [number(100 * p['recall'], 2), number(p['qps'])]
        provenance.append({'dataset': ds, 'method': method, 'curve': c['id'],
            'config': c['config'], 'parameters': c['parameters'], 'target': .98,
            'evidence': p['evidence']})
    rows.append(' & '.join(values) + r' \\')
(ROOT / 'tables/high-recall.tex').write_text('\n'.join(rows), encoding='utf-8')

rows = []
for ds in DATA['datasets']:
    c = max((c for c in CURVES if c['dataset'] == ds['id'] and c['method'] == 'QuIVer'),
            key=lambda c: c['parameters']['ef'])
    assert c['parameters']['ef'] == 1024
    p, cfg = c['points']['1'], CONFIG[c['config']]
    name = NAMES.get(ds['id'], ds['name'])
    values = [tex(name), number(ds['n']), str(ds['dim']), number(p['queries']),
              'L2' if ds['metric'] == 'euclidean' else 'Cosine',
              number(p['recall'] * 100, 2), number(p['qps']),
              number(preparation(cfg), 1), number(cfg['hot'], 3)]
    rows.append(' & '.join(values) + r' \\')
(ROOT / 'tables/datasets.tex').write_text('\n'.join(rows), encoding='utf-8')

# 全清单用原始身份展开为配套数据，保持所有可用测量位置。
with (ROOT / 'evidence/all-operating-points.csv').open('w', encoding='utf-8-sig', newline='') as f:
    writer = csv.writer(f)
    writer.writerow(['dataset', 'method', 'config', 'parameters', 'threads', 'recall',
        'qps', 'batch_qps', 'p95_ms', 'p99_ms', 'hot_gib', 'cold_gib', 'build_s', 'install_s', 'domain'])
    count = 0
    for c in CURVES:
        cfg = CONFIG[c['config']]
        for p in c['points'].values():
            writer.writerow([c['dataset'], c['method'], c['config'],
                json.dumps(c['parameters'], ensure_ascii=False), p['threads'], p['recall'],
                p['qps'], p['batch'], p['p95'], p['p99'], cfg['hot'], cfg['cold'],
                cfg['build'], cfg['install'], c['domain']])
            count += 1
    assert count == 1056
(ROOT / 'evidence/table-provenance.json').write_text(json.dumps(provenance, ensure_ascii=False, indent=2), encoding='utf-8')

# 图形沿用同一冻结快照；表格生成与视觉排版分开维护。
render_all()
print('已生成：四幅配图、三张主表、完整1056条观测及来源清单。')
