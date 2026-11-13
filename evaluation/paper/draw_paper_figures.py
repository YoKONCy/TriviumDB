"""重绘论文机制图及实测图；沿用冻结结果，保留SVG文字和矢量PDF。

数据图规范参考 scientific-visualization，doi:10.48550/arXiv.2609.00065。
流程构思参考位于 ccfa-workfiles/figures/pipeline，实际计算关系以正文为准。
"""
import json
import hashlib
from collections import defaultdict
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from matplotlib.lines import Line2D
from matplotlib.patches import Rectangle, FancyBboxPatch, FancyArrowPatch, Circle
from matplotlib.ticker import FuncFormatter, LogLocator, NullFormatter
import numpy as np

ROOT = Path(__file__).resolve().parent
CORE = ['D03', 'D05', 'D14']
NAMES = {'D03': 'Cohere-1M', 'D05': 'DBpedia-1536', 'D14': 'Cohere-5M'}
WIDTH = 7.125
INK = '#22313b'
MUTED = '#596d7a'
GRID = '#e4e9ed'
TEAL = '#087f8c'
BLUE = '#3a67b0'
AMBER = '#bf5b17'
METHODS = {
    'QuIVer': ('QuIVer', TEAL, 'o'),
    'SymphonyQG': ('SymphonyQG', AMBER, 's'),
    'NGT-QG': ('NGT-QG', '#8e6bbd', 'D'),
    'Lucene BBQ + FP32': ('Lucene BBQ + FP32', BLUE, '^'),
    'SVS LVQ8': ('SVS LVQ8', '#ba5276', 'v'),
    'VSAG HGraph SQ8 + FP32': ('VSAG SQ8 + FP32', '#747c31', 'P'),
    'HNSWlib FP32': ('HNSWlib FP32', '#56616d', 'X'),
    'DiskANN FP32': ('DiskANN FP32', '#aa8b00', 'h'),
    'RaBitQ 官方 IVF + FP32': ('RaBitQ IVF + FP32', '#358059', '*'),
}
STYLES = ['-', '--', '-.', ':', (0, (4, 1, 1, 1)), (0, (5, 2)),
          (0, (2, 1)), (0, (6, 1, 1, 1)), (0, (1, 1))]
STYLE = dict(zip(METHODS, STYLES))
THREAD_METHODS = ['QuIVer', 'SymphonyQG', 'Lucene BBQ + FP32', 'SVS LVQ8', 'HNSWlib FP32']
STYLE_RC = {
    'font.family': 'Arial', 'font.size': 8.6, 'axes.labelsize': 8.8,
    'axes.titlesize': 10.2, 'axes.titleweight': 'bold',
    'xtick.labelsize': 8, 'ytick.labelsize': 8,
    'legend.fontsize': 8, 'text.color': INK, 'axes.labelcolor': INK,
    'axes.edgecolor': '#85939d', 'axes.linewidth': .6,
    'axes.spines.top': False, 'axes.spines.right': False,
    'xtick.color': MUTED, 'ytick.color': MUTED,
    'xtick.major.width': .6, 'ytick.major.width': .6,
    'xtick.major.size': 3, 'ytick.major.size': 3,
    'pdf.fonttype': 42, 'ps.fonttype': 42, 'svg.fonttype': 'none',
    'mathtext.fontset': 'dejavusans', 'savefig.facecolor': 'white',
}
RECORD = {}


def compact(value, _pos=None):
    if value >= 1000:
        return f'{value/1000:g}k'
    return f'{value:g}'


def target_curve(data, dataset, method, target=.95):
    matches = [c for c in data['curves'] if c['dataset'] == dataset and c['method'] == method
               and any(t['target'] == target for t in c.get('targets', []))
               and c['points'].get('1', {}).get('eligible', True)
               and c['points'].get('1', {}).get('qps')]
    if len(matches) != 1:
        raise ValueError(f'冻结选择不唯一：{dataset} / {method} / {target}')
    return matches[0]


def base_axes(ax, title=None, logarithmic=False):
    ax.set_axisbelow(True)
    ax.grid(axis='y', which='major', color=GRID, lw=.65)
    ax.grid(axis='x', which='major', color=GRID, lw=.45, alpha=.75)
    if logarithmic:
        ax.set_yscale('log')
        ax.yaxis.set_major_formatter(FuncFormatter(compact))
        ax.yaxis.set_minor_locator(LogLocator(base=10, subs=[2, 5]))
        ax.yaxis.set_minor_formatter(NullFormatter())
    if title:
        ax.set_title(title, loc='left', pad=9)


def legend(fig, methods, others=False, ideal=False):
    handles = [Line2D([], [], label=METHODS[m][0], color=METHODS[m][1],
                      marker=METHODS[m][2], ls=STYLE[m], ms=4,
                      lw=1.9 if m == 'QuIVer' else 1) for m in methods]
    if others:
        handles.append(Line2D([], [], label='Other measured modes', color='#aeb9c1',
                              marker='.', ms=3, lw=.65))
    if ideal:
        handles.append(Line2D([], [], label='Linear scaling', color='#a5afb5', ls='--', lw=.8))
    fig.legend(handles=handles, loc='lower center', bbox_to_anchor=(.53, .012),
               ncol=4 if others else 3, frameon=False, columnspacing=1.0, handlelength=2.0,
               handletextpad=.55, labelspacing=.65)


def save(fig, name, records):
    # 输出的物理画幅固定，避免tight裁切悄悄改变论文中嵌入后的字号。
    for extension in ['svg', 'pdf', 'png']:
        fig.savefig(ROOT / 'figures' / f'{name}.{extension}', dpi=600, facecolor='white')
    # 保存高DPI图片后重新绘制屏幕画布，避免混用两套坐标进行检查。
    fig.canvas.draw()
    renderer = fig.canvas.get_renderer()
    width, height = fig.canvas.get_width_height()
    outside = []
    texts = list(fig.texts)
    for ax in fig.axes:
        texts.extend(list(ax.texts) + [ax.title, ax.xaxis.label, ax.yaxis.label])
    for item in fig.legends:
        texts.extend(item.get_texts())
    for text in texts:
        if not text.get_visible() or not text.get_text().strip():
            continue
        box = text.get_window_extent(renderer)
        if box.x0 < -1 or box.y0 < -1 or box.x1 > width + 1 or box.y1 > height + 1:
            outside.append(text.get_text())
    RECORD[name] = {'width_inches': WIDTH, 'height_inches': fig.get_figheight(),
                    'source_records': records, 'outside_canvas': outside}
    plt.close(fig)


def recall_plot(data):
    fig, axes = plt.subplots(1, 3, figsize=(WIDTH, 3.15), sharex=True, sharey=True)
    fig.subplots_adjust(left=.078, right=.985, top=.90, bottom=.34, wspace=.16)
    records = []
    for j, (ax, ds) in enumerate(zip(axes, CORE)):
        base_axes(ax, f'({chr(97+j)}) {NAMES[ds]}', logarithmic=True)
        grouped = defaultdict(list)
        for c in data['curves']:
            p = c['points'].get('1', {})
            if c['dataset'] == ds and p.get('qps') and p.get('eligible', True):
                grouped[c['config']].append(c)
        for config, cs in sorted(grouped.items(), key=lambda item: item[1][0]['method'] == 'QuIVer'):
            cs.sort(key=lambda c: c['points']['1']['recall'])
            method = cs[0]['method']
            selected = method in METHODS
            color = METHODS[method][1] if selected else '#aeb9c1'
            marker = METHODS[method][2] if selected else '.'
            x = [100*c['points']['1']['recall'] for c in cs]
            y = [c['points']['1']['qps'] for c in cs]
            ax.plot(x, y, color=color, marker=marker, ls=STYLE.get(method, '-'),
                    ms=4.0 if selected else 2.1, mew=.45,
                    lw=1.95 if method == 'QuIVer' else .9 if selected else .55,
                    alpha=1 if selected else .58, zorder=6 if method == 'QuIVer' else 3 if selected else 1)
            records.extend({'curve': c['id'], 'config': config, 'threads': 1,
                            'recall': c['points']['1']['recall'], 'qps': c['points']['1']['qps']}
                           for c in cs)
        c = target_curve(data, ds, 'QuIVer')
        p = c['points']['1']
        ax.scatter([100*p['recall']], [p['qps']], s=47, facecolors='none',
                   edgecolors=TEAL, linewidths=.9, zorder=7)
        ax.annotate('QuIVer', xy=(100*p['recall'], p['qps']), xytext=(-21, 17),
                    textcoords='offset points', ha='right', fontsize=8.2, color=TEAL,
                    fontweight='bold', arrowprops={'arrowstyle': '-', 'color': TEAL, 'lw': .7},
                    bbox={'facecolor': 'white', 'edgecolor': 'none', 'alpha': .88, 'pad': .35})
        ax.set(xlim=(88, 100.2), ylim=(5, 24000), xticks=[90, 95, 100], xlabel='Recall@10 (%)')
    axes[0].set_ylabel('Single-thread throughput (QPS)')
    legend(fig, list(METHODS), others=True)
    save(fig, 'recall-throughput', records)


def deployment_plot(data, configs):
    fig, axes = plt.subplots(2, 3, figsize=(WIDTH, 4.15), sharey=True)
    fig.subplots_adjust(left=.078, right=.985, top=.92, bottom=.24, hspace=.45, wspace=.16)
    records = []
    label_offsets = {'D03': (8, 11), 'D05': (8, 11), 'D14': (8, 11)}
    for j, ds in enumerate(CORE):
        for row in (0, 1):
            ax = axes[row, j]
            base_axes(ax, f'({chr(97+j)}) {NAMES[ds]}' if row == 0 else None, logarithmic=True)
            ax.set_xscale('log')
            ax.xaxis.set_major_formatter(FuncFormatter(compact))
            ax.xaxis.set_minor_formatter(NullFormatter())
            for method, (label, color, marker) in METHODS.items():
                c = target_curve(data, ds, method)
                p, cfg = c['points']['1'], configs[c['config']]
                # 总体内存折中采用逻辑组件合计：导航组件加独立原向量库。
                value = cfg['hot'] + cfg['cold'] if row == 0 else cfg['build'] + (cfg.get('install') or 0)
                assert value > 0 and p['qps'] > 0
                ax.scatter(value, p['qps'], marker=marker, color=color,
                           s=35 if method == 'QuIVer' else 27, edgecolor='white',
                           linewidth=.45, zorder=6 if method == 'QuIVer' else 3)
                records.append({'curve': c['id'], 'config': c['config'], 'threads': 1,
                                'metric': 'total_logical_gib' if row == 0 else 'preparation_s',
                                'x': value, 'qps': p['qps'], 'recall': p['recall']})
                if method == 'QuIVer':
                    suffix = f'{value:.2f} GiB' if row == 0 else f'{value:,.0f} s'
                    # 合计内存行右上方有高内存方法，标签改放在点的左上方。
                    ax.annotate(f'QuIVer\n{suffix}', (value, p['qps']),
                                xytext=label_offsets[ds] if row else (-10, 12), textcoords='offset points',
                                ha='left' if row else 'right',
                                va='bottom', fontsize=7.8, linespacing=1.2, color=TEAL, fontweight='bold',
                                bbox={'facecolor': 'white', 'edgecolor': 'none', 'alpha': .88, 'pad': .35},
                                arrowprops={'arrowstyle': '-', 'lw': .55, 'color': TEAL})
            ax.set(ylim=(120, 25000), xlim=(.5, 80) if row == 0 else (40, 25000))
            ax.set_xlabel('Total logical memory (GiB)' if row == 0 else 'Index preparation (s)')
            ax.set_xticks([1, 10] if row == 0 else [100, 1000, 10000])
    # 两行使用同一吞吐量尺度，采用共享纵轴标题避免相邻长标题重叠。
    fig.supylabel('Single-thread throughput (QPS)', x=.012, fontsize=8.8)
    legend(fig, list(METHODS))
    save(fig, 'memory-throughput', records)


def concurrency_plot(data):
    fig, axes = plt.subplots(2, 3, figsize=(WIDTH, 3.80), sharex=True, sharey='row')
    fig.subplots_adjust(left=.078, right=.985, top=.91, bottom=.22, hspace=.30, wspace=.16)
    records = []
    threads = [1, 8, 16, 32]
    for j, ds in enumerate(CORE):
        for row in (0, 1):
            ax = axes[row, j]
            base_axes(ax, f'({chr(97+j)}) {NAMES[ds]}' if row == 0 else None)
            for method in THREAD_METHODS:
                c = target_curve(data, ds, method)
                one = c['points']['1']['qps']
                assert all(str(t) in c['points'] for t in threads)
                absolute = [c['points'][str(t)]['qps'] for t in threads]
                y = absolute if row == 0 else [value/one for value in absolute]
                ax.plot(threads, y, color=METHODS[method][1], marker=METHODS[method][2],
                        ls=STYLE[method], lw=1.85 if method == 'QuIVer' else 1.05,
                        ms=3.6, mew=.5, zorder=6 if method == 'QuIVer' else 3)
                for t, value in zip(threads, y):
                    records.append({'curve': c['id'], 'config': c['config'], 'threads': t,
                                    'metric': 'qps' if row == 0 else 'speedup', 'value': value,
                                    'qps': c['points'][str(t)]['qps']})
                if method == 'QuIVer':
                    text = f'{absolute[-1]/1000:.1f}k QPS' if row == 0 else f'{y[-1]:.2f}'+r'$\times$'
                    if row == 0:
                        ax.annotate(text, (32, y[-1]), xytext=(-3, 9), textcoords='offset points',
                                    ha='right', color=TEAL, fontsize=8, fontweight='bold',
                                    bbox={'facecolor': 'white', 'edgecolor': 'none', 'alpha': .88, 'pad': .35})
                    else:
                        ax.text(.05, .91, 'QuIVer: '+text, transform=ax.transAxes,
                                ha='left', va='top', color=TEAL, fontsize=8, fontweight='bold')
            ax.set(xlim=(0, 34), xticks=threads)
            if row == 0:
                ax.set_ylim(0, 190000)
                ax.set_yticks([0, 50000, 100000, 150000])
                ax.yaxis.set_major_formatter(FuncFormatter(compact))
            else:
                ax.plot([1, 32], [1, 32], color='#a5afb5', ls='--', lw=.7, zorder=1)
                ax.set(ylim=(0, 34), yticks=[0, 8, 16, 24, 32], xlabel='Query threads')
    axes[0, 0].set_ylabel('Throughput (QPS)')
    axes[1, 0].set_ylabel('Speedup over one thread')
    legend(fig, THREAD_METHODS, ideal=True)
    save(fig, 'thread-scaling', records)


def pipeline_plot():
    fig, ax = plt.subplots(figsize=(WIDTH, 3.32))
    fig.subplots_adjust(left=0, right=1, top=1, bottom=0)
    ax.set(xlim=(0, 1800), ylim=(820, 0))
    ax.axis('off')

    def txt(x, y, text, size=8.6, color=INK, weight='normal', ha='center', **kwargs):
        return ax.text(x, y, text, fontsize=size, color=color, weight=weight,
                       ha=ha, va='center', **kwargs)

    def arrow(points, color=MUTED, lw=.8, dashed=False):
        for a, b in zip(points[:-2], points[1:-1]):
            ax.plot([a[0], b[0]], [a[1], b[1]], color=color, lw=lw,
                    ls='--' if dashed else '-', solid_capstyle='round')
        ax.add_patch(FancyArrowPatch(points[-2], points[-1], arrowstyle='-|>',
                                     mutation_scale=7, color=color, lw=lw,
                                     linestyle='--' if dashed else '-'))

    def matrix(x, y, rows, cols, w, h, color, bits=None, gid='matrix'):
        for i in range(rows):
            for j in range(cols):
                value = bits[i][j] if bits is not None else None
                ax.add_patch(Rectangle((x+j*w, y+i*h), w, h,
                                      fc=color if value or bits is None else '#f4f7f8', ec='white',
                                      lw=.5, gid=f'{gid}-{i}-{j}'))
                if value is not None:
                    txt(x+(j+.5)*w, y+(i+.5)*h, str(value), size=6.8,
                        color='white' if value else MUTED)

    def graph(cx, cy, scale=1, highlighted=False):
        coordinates=np.array([[-115, 4], [-65, -67], [18, -95], [98, -40],
                              [112, 49], [30, 94], [-63, 72], [-8, -1]])*scale
        pairs=[(0,1),(0,6),(0,7),(1,2),(1,7),(2,3),(2,7),(3,4),
               (3,7),(4,5),(4,7),(5,6),(5,7),(6,7)]
        for a,b in pairs:
            aa,bb=coordinates[a]+[cx,cy],coordinates[b]+[cx,cy]
            ax.plot([aa[0],bb[0]],[aa[1],bb[1]], color='#bdcbd3',lw=.65,zorder=1)
        if highlighted:
            for a,b in [(0,7),(7,3),(3,4)]:
                aa,bb=coordinates[a]+[cx,cy],coordinates[b]+[cx,cy]
                arrow([tuple(aa),tuple(bb)],TEAL,1.4)
        for i,(x,y) in enumerate(coordinates):
            active=highlighted and i in [0,7,3,4]
            ax.add_patch(Circle((x+cx,y+cy),12*scale,fc=TEAL if active else '#e5edf6',
                                ec=TEAL if active else BLUE,lw=.75,zorder=4))

    # 图形元素表现计算对象；两条横向带区分一次性构建与逐查询工作。
    ax.add_patch(Rectangle((22, 85), 1388, 303, fc='#f2f8f8', ec='none', gid='construction-region'))
    ax.add_patch(Rectangle((22, 495), 1756, 288, fc='#f4f7fb', ec='none', gid='query-region'))
    txt(42, 44, '(a) One-time index preparation', size=10, weight='bold', ha='left')
    txt(42, 450, '(b) Each query', size=10, weight='bold', ha='left')

    matrix(95, 151, 4, 5, 22, 36, '#c0ccd3', gid='original-coordinates')
    txt(150, 119, 'Original vectors X', size=8.6, weight='bold')
    txt(150, 330, 'FP32 coordinates', size=8)
    arrow([(225, 225), (355, 225)], TEAL)
    txt(289, 280, 'Encode', size=8)
    matrix(379, 149, 2, 6, 32, 32, TEAL,
           bits=[[1,0,1,1,0,1],[0,1,1,0,1,0]], gid='database-bit-planes')
    txt(475, 116, 'Two coordinate bits', size=8.7, weight='bold')
    txt(360, 165, r'$p_x$', size=8.6, ha='right')
    txt(360, 197, r'$h_x$', size=8.6, ha='right')
    txt(475, 289, 'SM metadata', size=8)
    txt(475, 333, r'$a_x,\ b_x,\ \|\widehat{x}\|^2$', size=9.6, color=TEAL)

    arrow([(596, 180), (772, 180)], BLUE)
    txt(683, 144, 'BQ scores', size=8.2, color=BLUE)
    txt(682, 249, 'Parallel builder', size=7.9)
    graph(964, 243, .85)
    txt(964, 116, 'Shared graph G', size=8.8, weight='bold')
    txt(964, 354, 'Integer construction proxy', size=8, color=BLUE)
    txt(1275, 192, 'Navigation state', size=8.5, weight='bold')
    txt(1275, 234, 'Graph + bit planes', size=8)
    txt(1275, 270, '+ SM rows + ID maps', size=8)

    # 原始向量仓库只连接最终精排；色彩不表示页面是否实际驻留。
    ax.add_patch(Rectangle((1481, 90), 292, 295, fc='#fff5e9', ec='none', gid='fp32-store'))
    txt(1626, 120, 'Original-vector store', size=8.6, weight='bold', color=AMBER)
    matrix(1549, 170, 3, 5, 30, 36, '#d6b995', gid='fp32-rerank-coordinates')
    txt(1626, 324, 'FP32 values', size=8.5, color=AMBER)

    arrow([(476, 375), (476, 475), (550, 475), (550, 535)], TEAL, dashed=True)
    txt(400, 411, 'SM rows', size=8.2, color=TEAL)
    arrow([(964, 375), (964, 475), (959, 535)], BLUE, dashed=True)
    txt(1038, 416, 'Adjacency', size=8.2, color=BLUE)
    arrow([(1626, 385), (1626, 501), (1572, 501), (1572, 585)], AMBER, 1)

    txt(166, 541, 'Encode query q', size=8.5, weight='bold')
    matrix(77, 580, 2, 6, 28, 30, TEAL,
           bits=[[1,0,1,0,1,1],[0,1,0,1,1,0]], gid='query-bit-planes')
    txt(61, 595, r'$p_q$', size=8.3, ha='right')
    txt(61, 625, r'$h_q$', size=8.3, ha='right')
    txt(162, 677, r'$a_q,\ b_q,\ \|\widehat q\|^2$', size=9.2, color=TEAL)
    arrow([(265, 616), (350, 616)], TEAL, 1.1)

    txt(550, 550, 'Norm-aware bitwise score', size=8.6, weight='bold')
    for x,label in [(380, 'XOR'), (484, 'AND'), (588, 'popcnt')]:
        ax.add_patch(FancyBboxPatch((x, 590), 97, 53, boxstyle='round,pad=0,rounding_size=4',
                                    ec=TEAL, fc='#e2f0ef', lw=.7, gid=f'score-operator-{label}'))
        txt(x+48.5, 616, label, size=8.2, color=TEAL)
    txt(550, 703, r'$D_{\rm SM}=\|\widehat q\|^2+\|\widehat x\|^2-2\langle\widehat q,\widehat x\rangle$',
        size=9.1, color=TEAL)
    # XOR、AND和popcnt是评分内核中的运算种类，不画成错误的串行公式。
    arrow([(711, 616), (803, 616)], TEAL, 1.1)
    graph(932, 627, .85, highlighted=True)
    txt(943, 748, 'Navigate + retain', size=8.3, weight='bold')
    arrow([(1049, 616), (1120, 616)], TEAL, 1.1)

    txt(1197, 548, 'Final beam C', size=8.5, weight='bold')
    for i in range(5):
        ax.add_patch(Rectangle((1145, 578+i*27), 104, 21, fc=TEAL if i<3 else '#cad5e2',
                               ec='none', gid=f'beam-candidate-{i}'))
    txt(1197, 745, 'width ef', size=8.3)
    arrow([(1268, 616), (1470, 616)], BLUE, 1.1)
    txt(1366, 552, 'Select F', size=8.5, weight='bold', color=BLUE)
    txt(1366, 696, r'$|F|\leq\min(64,ef)$', size=8.5, color=BLUE)

    ax.add_patch(FancyBboxPatch((1476, 586), 197, 83, boxstyle='round,pad=0,rounding_size=5',
                                fc='#fff2df', ec=AMBER, lw=.8, gid='exact-rerank'))
    txt(1575, 609, 'Exact FP32', size=8.8, weight='bold', color=AMBER)
    txt(1575, 643, 'rerank', size=8.8, weight='bold', color=AMBER)
    arrow([(1675, 628), (1715, 628)], AMBER, .9)
    txt(1743, 589, 'Top', size=8.4)
    txt(1743, 630, '10', size=11, weight='bold')
    txt(1576, 745, 'Original task metric', size=8.2, color=AMBER)
    save(fig, 'pipeline', [{'source': 'sections/design.tex',
                           'content': '编码、整数构建、SM-L2导航、有限候选和FP32精排；比特矩阵与图节点是机制示意。'}])


def render_all():
    data_path = ROOT.parent / 'results/benchmark-snapshot.json'
    data = json.loads(data_path.read_text(encoding='utf-8-sig'))
    configs = {c['id']: c for c in data['configs']}
    with plt.rc_context(STYLE_RC):
        recall_plot(data)
        deployment_plot(data, configs)
        concurrency_plot(data)
        pipeline_plot()
    report = {'data_sha256': hashlib.sha256(data_path.read_bytes()).hexdigest(),
              'numeric_transforms': ['召回乘100显示百分比', '多线程QPS除以同图同参数1T QPS',
                                     '准备时间为记录的build加install；缺省install不额外添加阶段'],
              'recall_focus_percent': [88, 100.2], 'smoothing': False,
              'curve_identity': '仅连接同一config；其他已测模式用灰色保留',
              'selection': '复用原先冻结的95%调参选择，不按评估结果重新选点',
              'uncertainty': '图中为原记录的五轮平均吞吐，不由CV构造置信区间',
              'font': 'Arial，与现有理论图保持一致', 'figures': RECORD}
    (ROOT / 'evidence/figure-provenance.json').write_text(
        json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
    print('已更新四幅论文配图：可编辑SVG、矢量PDF与600 dpi PNG。')


if __name__ == '__main__':
    import sys
    sys.stdout.reconfigure(encoding='utf-8')
    render_all()
