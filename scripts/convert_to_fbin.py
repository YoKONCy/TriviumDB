"""
将裸 .f32 / .i32 文件转换为 DiskANN 的 .fbin / .ibin 格式（8 字节 header: nrows + ndims）
"""
import struct
import sys
import os

def convert(input_path, output_path, nrows, ndims, dtype_size=4):
    """在文件头部添加 nrows 和 ndims"""
    input_size = os.path.getsize(input_path)
    expected = nrows * ndims * dtype_size
    assert input_size == expected, f"文件大小不匹配: {input_size} != {expected} (nrows={nrows}, ndims={ndims})"
    
    with open(output_path, 'wb') as fout:
        # 写入 header
        fout.write(struct.pack('<i', nrows))
        fout.write(struct.pack('<i', ndims))
        # 流式拷贝数据
        with open(input_path, 'rb') as fin:
            chunk_size = 64 * 1024 * 1024  # 64MB
            while True:
                data = fin.read(chunk_size)
                if not data:
                    break
                fout.write(data)
    
    final_size = os.path.getsize(output_path)
    print(f"[OK] {output_path}: {final_size} bytes (header 8 + data {expected})")

if __name__ == "__main__":
    base = r"c:\Users\Administrator\OneDrive\桌面\workspace\TriviumDB"
    out_dir = os.path.join(base, "diskann_data")
    os.makedirs(out_dir, exist_ok=True)
    
    # 训练集: 1000000 x 768 float32
    print("转换 cohere_train.f32 → data.fbin ...")
    convert(
        os.path.join(base, "cohere_train.f32"),
        os.path.join(out_dir, "data.fbin"),
        nrows=1000000, ndims=768
    )
    
    # 测试集: 1000 x 768 float32
    print("转换 cohere_test.f32 → queries.fbin ...")
    convert(
        os.path.join(base, "cohere_test.f32"),
        os.path.join(out_dir, "queries.fbin"),
        nrows=1000, ndims=768
    )
    
    # GT: 1000 x 1000 int32 → 只取前 10 列给 DiskANN（它用 recall@10）
    # 但 DiskANN 的 ibin 格式实际上可以接受更多列，所以直接转全部
    print("转换 cohere_groundtruth.i32 → groundtruth.ibin ...")
    convert(
        os.path.join(base, "cohere_groundtruth.i32"),
        os.path.join(out_dir, "groundtruth.ibin"),
        nrows=1000, ndims=1000
    )
    
    print("\n全部转换完成！")
