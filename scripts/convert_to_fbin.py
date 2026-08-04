"""
将裸 .f32 / .i32 文件转换为 DiskANN 的 .fbin / .ibin 格式（8 字节 header: nrows + ndims）
"""
import argparse
import struct
from pathlib import Path

def convert(input_path, output_path, nrows, ndims, dtype_size=4):
    """在文件头部添加 nrows 和 ndims"""
    input_size = input_path.stat().st_size
    expected = nrows * ndims * dtype_size
    assert input_size == expected, f"文件大小不匹配: {input_size} != {expected} (nrows={nrows}, ndims={ndims})"
    
    with output_path.open('wb') as fout:
        # 写入 header
        fout.write(struct.pack('<i', nrows))
        fout.write(struct.pack('<i', ndims))
        # 流式拷贝数据
        with input_path.open('rb') as fin:
            chunk_size = 64 * 1024 * 1024  # 64MB
            while True:
                data = fin.read(chunk_size)
                if not data:
                    break
                fout.write(data)
    
    final_size = output_path.stat().st_size
    print(f"[OK] {output_path}: {final_size} bytes (header 8 + data {expected})")

def parse_args():
    parser = argparse.ArgumentParser(description="将 Cohere raw binary 数据转换为 DiskANN 格式")
    parser.add_argument("--input-dir", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--train-rows", type=int, default=1_000_000)
    parser.add_argument("--dim", type=int, default=768)
    return parser.parse_args()


def main():
    args = parse_args()
    base = args.input_dir.resolve()
    out_dir = (args.output_dir or base / "diskann_data").resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    train_path = base / "cohere_train.f32"
    query_path = base / "cohere_test.f32"
    gt_path = base / "cohere_groundtruth.i32"
    query_row_bytes = args.dim * 4
    query_size = query_path.stat().st_size
    if query_size % query_row_bytes != 0:
        raise ValueError(f"查询文件大小无法按 {args.dim} 维 float32 向量解析: {query_size}")
    query_rows = query_size // query_row_bytes
    gt_row_bytes = query_rows * 4
    gt_size = gt_path.stat().st_size
    if gt_size % gt_row_bytes != 0:
        raise ValueError(f"Ground truth 文件大小无法按 {query_rows} 行 int32 解析: {gt_size}")
    gt_dims = gt_size // gt_row_bytes
    
    # 训练集
    print("转换 cohere_train.f32 → data.fbin ...")
    convert(
        train_path,
        out_dir / "data.fbin",
        nrows=args.train_rows, ndims=args.dim
    )
    
    # 测试集
    print("转换 cohere_test.f32 → queries.fbin ...")
    convert(
        query_path,
        out_dir / "queries.fbin",
        nrows=query_rows, ndims=args.dim
    )
    
    # Ground truth 的行数和列数根据输入文件推导
    print("转换 cohere_groundtruth.i32 → groundtruth.ibin ...")
    convert(
        gt_path,
        out_dir / "groundtruth.ibin",
        nrows=query_rows, ndims=gt_dims
    )
    
    print("\n全部转换完成！")


if __name__ == "__main__":
    main()
