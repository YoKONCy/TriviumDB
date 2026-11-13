//! 以独立字节样例和重建向量的余弦定义验证编码及整数评分。
use super::*;

const GOLDEN_HEADER: [u8; 40] = [
    81, 86, 67, 79, 76, 68, 48, 49, 1, 0, 0, 0, 1, 0, 0, 0, 4, 0, 0, 0, 128, 0, 0, 0, 20, 0, 0, 0,
    32, 0, 0, 0, 0, 0, 0, 0, 85, 100, 242, 153,
];
// [1,-1,1,-1]归一化后为[0.5,-0.5,0.5,-0.5]；CRC由独立zlib工具计算。
const GOLDEN_ROW: [u8; 32] = [
    127, 129, 127, 129, 4, 2, 129, 59, 0, 0, 0, 0, 0, 0, 128, 63, 252, 68, 96, 248, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0,
];

fn encoded<T: VectorType>(layout: Int8Layout, vector: &[T]) -> Vec<u8> {
    let mut row = vec![0xa5; layout.row_stride_bytes()];
    layout.encode(vector, &mut row).unwrap();
    row
}

fn checksum_row(layout: Int8Layout, bytes: &mut [u8]) {
    let end = layout.payload_bytes() - 4;
    let crc = crc32fast::hash(&bytes[..end]);
    bytes[end..end + 4].copy_from_slice(&crc.to_le_bytes());
}

fn cosine(left: &[f64], right: &[f64]) -> f64 {
    let dot = left.iter().zip(right).map(|(a, b)| a * b).sum::<f64>();
    let norm_left = left.iter().map(|value| value * value).sum::<f64>().sqrt();
    let norm_right = right.iter().map(|value| value * value).sum::<f64>().sqrt();
    if norm_left == 0. || norm_right == 0. {
        0.
    } else {
        dot / norm_left / norm_right
    }
}

/// 直接构造已知重建值的行，绕开被测编码器和解码器。
fn reference_row(dimension: usize) -> (Int8Layout, Vec<u8>, Vec<f64>) {
    let layout = Int8Layout::new(dimension, 64).unwrap();
    let mut bytes = vec![0; layout.row_stride_bytes()];
    let mut values = Vec::with_capacity(dimension);
    for (index, target) in bytes[..dimension].iter_mut().enumerate() {
        let code = (index % 9) as i8 - 3;
        let bias = if index / 128 % 2 == 0 { 0.5 } else { -0.25 };
        *target = code as u8;
        values.push(f64::from(code) / 8. + bias);
    }
    for group in 0..layout.groups() {
        let offset = dimension + group * 8;
        let bias: f32 = if group % 2 == 0 { 0.5 } else { -0.25 };
        bytes[offset..offset + 4].copy_from_slice(&0.125f32.to_le_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&bias.to_le_bytes());
    }
    let norm = values.iter().map(|value| value * value).sum::<f64>().sqrt();
    let factor = (1. / norm) as f32;
    bytes[layout.payload_bytes() - 8..layout.payload_bytes() - 4]
        .copy_from_slice(&factor.to_le_bytes());
    checksum_row(layout, &mut bytes);
    (layout, bytes, values)
}

#[test]
fn golden_layout_and_row_match_independent_bytes() {
    let layout = Int8Layout::new(4, 16).unwrap();
    assert_eq!(layout.to_header(), GOLDEN_HEADER);
    assert_eq!(Int8Layout::from_header(&GOLDEN_HEADER).unwrap(), layout);
    assert_eq!(encoded(layout, &[1f32, -1., 1., -1.]), GOLDEN_ROW);
    let row = layout.row(&GOLDEN_ROW).unwrap();
    assert_eq!(row.layout(), layout);
    let mut decoded = [0f32; 4];
    row.decode_into(&mut decoded).unwrap();
    assert_eq!(decoded, [0.5, -0.5, 0.5, -0.5]);
    assert_eq!(
        Int8Query::new(&[1f32, -1., 1., -1.])
            .unwrap()
            .score(row)
            .unwrap(),
        1.
    );
}

#[test]
fn dynamic_layout_has_explicit_crc_and_stride() {
    // 有效载荷包含4字节行CRC，字节数由格式定义直接给出。
    for (dim, groups, payload, stride) in [
        (1, 1, 17, 64),
        (384, 3, 416, 448),
        (768, 6, 824, 832),
        (769, 7, 833, 896),
        (1024, 8, 1096, 1152),
        (1536, 12, 1640, 1664),
        (3072, 24, 3272, 3328),
    ] {
        let layout = Int8Layout::new(dim, 64).unwrap();
        assert_eq!(
            (
                layout.dimension(),
                layout.groups(),
                layout.payload_bytes(),
                layout.row_stride_bytes()
            ),
            (dim, groups, payload, stride)
        );
        assert_eq!(
            Int8Layout::from_header(&layout.to_header()).unwrap(),
            layout
        );
    }
    assert_eq!(Int8Layout::new(768, 1024).unwrap().row_stride_bytes(), 1024);
}

#[test]
fn row_offsets_check_bounds_and_overflow() {
    let layout = Int8Layout::new(768, 1024).unwrap();
    assert_eq!(layout.rows_bytes(0).unwrap(), 0);
    assert_eq!(layout.rows_bytes(3).unwrap(), 3072);
    assert_eq!(layout.row_range(2, 3).unwrap(), 2048..3072);
    for (row, rows) in [(0, 0), (3, 3), (0, u64::MAX), (u64::MAX, u64::MAX)] {
        assert_eq!(
            layout.row_range(row, rows).unwrap_err().code(),
            "TDB_INVALID_INPUT"
        );
    }
}

#[test]
fn layout_rejects_invalid_dimensions_and_alignments() {
    for dim in [0, MAX_BQ_DIM + 1, usize::MAX] {
        assert_eq!(
            Int8Layout::new(dim, 64).unwrap_err().info().target,
            Some(crate::error::ErrorTarget::Field {
                field: "dimension".into()
            })
        );
    }
    for alignment in [0, 3, 63, 1usize << (usize::BITS - 1)] {
        let error = Int8Layout::new(768, alignment).unwrap_err();
        assert_eq!(error.code(), "TDB_INVALID_INPUT");
        assert_eq!(
            error.info().target,
            Some(crate::error::ErrorTarget::Field {
                field: "row_alignment".into()
            })
        );
    }
}

#[test]
fn every_header_byte_and_truncation_is_checked() {
    for length in 0..GOLDEN_HEADER.len() {
        assert_eq!(
            Int8Layout::from_header(&GOLDEN_HEADER[..length])
                .unwrap_err()
                .code(),
            "TDB_CORRUPTED_FILE"
        );
    }
    let mut extended = GOLDEN_HEADER.to_vec();
    extended.push(0);
    assert!(Int8Layout::from_header(&extended).is_err());
    for index in 0..GOLDEN_HEADER.len() {
        let mut changed = GOLDEN_HEADER;
        changed[index] ^= 1;
        assert_eq!(
            Int8Layout::from_header(&changed).unwrap_err().code(),
            "TDB_CORRUPTED_FILE",
            "字节{index}"
        );
    }
}

#[test]
fn checksummed_header_rejects_invalid_semantics() {
    for (offset, value) in [
        (8, 2u32),
        (12, 2),
        (16, 0),
        (16, 3073),
        (20, 64),
        (24, 19),
        (28, 19),
        (32, 1),
    ] {
        let mut header = GOLDEN_HEADER;
        header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        let checksum = crc32fast::hash(&header[..36]);
        header[36..].copy_from_slice(&checksum.to_le_bytes());
        assert_eq!(
            Int8Layout::from_header(&header).unwrap_err().code(),
            "TDB_CORRUPTED_FILE",
            "偏移{offset} 值{value}"
        );
    }
}

#[test]
fn every_row_byte_and_truncation_is_checked() {
    let layout = Int8Layout::from_header(&GOLDEN_HEADER).unwrap();
    for length in 0..GOLDEN_ROW.len() {
        assert_eq!(
            layout.row(&GOLDEN_ROW[..length]).unwrap_err().code(),
            "TDB_CORRUPTED_FILE"
        );
    }
    let mut extended = GOLDEN_ROW.to_vec();
    extended.push(0);
    assert!(layout.row(&extended).is_err());
    for index in 0..GOLDEN_ROW.len() {
        let mut bytes = GOLDEN_ROW;
        bytes[index] ^= 1;
        assert_eq!(
            layout.row(&bytes).unwrap_err().code(),
            "TDB_CORRUPTED_FILE",
            "字节{index}"
        );
    }
}

#[test]
fn checksummed_row_rejects_invalid_numeric_fields() {
    let layout = Int8Layout::from_header(&GOLDEN_HEADER).unwrap();
    for (offset, value) in [
        (4, f32::NAN),
        (4, f32::INFINITY),
        (4, 0.),
        (4, -1.),
        (8, f32::NEG_INFINITY),
        (12, f32::NAN),
        (12, 0.),
        (12, -1.),
        (12, 1.5),
    ] {
        let mut bytes = GOLDEN_ROW;
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        checksum_row(layout, &mut bytes);
        assert_eq!(
            layout.row(&bytes).unwrap_err().code(),
            "TDB_CORRUPTED_FILE",
            "偏移{offset} 值{value}"
        );
    }
    let mut bytes = GOLDEN_ROW;
    bytes[0] = 128;
    checksum_row(layout, &mut bytes);
    assert_eq!(layout.row(&bytes).unwrap_err().code(), "TDB_CORRUPTED_FILE");
}

#[test]
fn rejected_input_preserves_output_buffer() {
    let layout = Int8Layout::new(4, 16).unwrap();
    for input in [vec![1f32; 3], vec![f32::NAN; 4], vec![f32::INFINITY; 4]] {
        let mut output = [0xa5; 32];
        let error = layout.encode(&input, &mut output).unwrap_err();
        assert!(matches!(
            error,
            TriviumError::DimensionMismatch { .. } | TriviumError::InvalidVector { .. }
        ));
        assert_eq!(output, [0xa5; 32]);
    }
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            Int8Query::new(&[value; 4]).err().unwrap().code(),
            "TDB_INVALID_VECTOR"
        );
    }
    for length in [0, 31, 33] {
        let mut output = vec![0xa5; length];
        assert_eq!(
            layout.encode(&[1f32; 4], &mut output).unwrap_err().code(),
            "TDB_INVALID_INPUT"
        );
        assert_eq!(output, vec![0xa5; length]);
    }
}

#[test]
fn binary_vectors_are_rejected_without_losing_integer_bits() {
    let layout = Int8Layout::new(4, 16).unwrap();
    let mut output = [0xa5; 32];
    let error = layout.encode(&[u64::MAX; 4], &mut output).unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        error.info().target,
        Some(crate::error::ErrorTarget::Field {
            field: "dtype".into()
        })
    );
    assert_eq!(output, [0xa5; 32]);
    assert_eq!(
        Int8Query::new(&[u64::MAX; 4]).err().unwrap().code(),
        "TDB_INVALID_INPUT"
    );
}

#[test]
fn extreme_finite_vectors_have_the_same_directional_encoding() {
    let layout = Int8Layout::new(4, 16).unwrap();
    for magnitude in [f32::MAX, f32::MIN_POSITIVE, f32::from_bits(1)] {
        let vector = [magnitude, -magnitude, magnitude, -magnitude];
        assert_eq!(encoded(layout, &vector), GOLDEN_ROW);
        assert_eq!(
            Int8Query::new(&vector)
                .unwrap()
                .score(layout.row(&GOLDEN_ROW).unwrap())
                .unwrap(),
            1.
        );
    }
}

#[test]
fn half_precision_input_uses_the_same_codec() {
    let layout = Int8Layout::new(4, 16).unwrap();
    for magnitude in [
        half::f16::ONE,
        half::f16::MAX,
        half::f16::MIN_POSITIVE,
        half::f16::from_bits(1),
    ] {
        let vector = [magnitude, -magnitude, magnitude, -magnitude];
        assert_eq!(encoded(layout, &vector), GOLDEN_ROW);
        assert_eq!(
            Int8Query::new(&vector)
                .unwrap()
                .score(layout.row(&GOLDEN_ROW).unwrap())
                .unwrap(),
            1.
        );
    }
}

#[test]
fn subnormal_group_ranges_keep_a_positive_representable_scale() {
    let layout = Int8Layout::new(129, 64).unwrap();
    let mut vector = vec![0f32; 129];
    vector[0] = f32::from_bits(2);
    vector[128] = 1.;
    let bytes = encoded(layout, &vector);
    assert_eq!(&bytes[129..133], &[1, 0, 0, 0]);
    assert_eq!(&bytes[133..137], &[1, 0, 0, 0]);
    assert_eq!(bytes[0], 1);
    assert!(bytes[1..128].iter().all(|&code| code == 255));
    let row = layout.row(&bytes).unwrap();
    let mut decoded = vec![0f32; 129];
    row.decode_into(&mut decoded).unwrap();
    assert_eq!(decoded, vector);
    assert_eq!(Int8Query::new(&vector).unwrap().score(row).unwrap(), 1.);
}

#[test]
fn zero_vectors_and_zero_queries_have_zero_scores() {
    for dim in [1, 129, 769, 3072] {
        let layout = Int8Layout::new(dim, 64).unwrap();
        let zero = vec![0f32; dim];
        let bytes = encoded(layout, &zero);
        let row = layout.row(&bytes).unwrap();
        let mut output = vec![1f32; dim];
        row.decode_into(&mut output).unwrap();
        assert_eq!(output, zero);
        assert_eq!(
            Int8Query::new(&vec![1f32; dim])
                .unwrap()
                .score(row)
                .unwrap(),
            0.
        );
        let (other_layout, other_bytes, _) = reference_row(dim);
        assert_eq!(
            Int8Query::new(&zero)
                .unwrap()
                .score(other_layout.row(&other_bytes).unwrap())
                .unwrap(),
            0.
        );
    }
}

#[test]
fn all_dimensions_keep_the_last_component_and_tail_bias() {
    for dim in [
        1, 2, 63, 64, 65, 127, 128, 129, 383, 384, 385, 767, 768, 769, 1024, 1536, 3072,
    ] {
        let layout = Int8Layout::new(dim, 64).unwrap();
        let mut vector = vec![0f32; dim];
        vector[dim - 1] = 1.;
        let bytes = encoded(layout, &vector);
        let row = layout.row(&bytes).unwrap();
        let mut decoded = vec![0f32; dim];
        row.decode_into(&mut decoded).unwrap();
        for (index, (actual, expected)) in decoded.iter().zip(&vector).enumerate() {
            assert!(
                (actual - expected).abs() < 1e-7,
                "维度{dim} 分量{index} 实际{actual} 预期{expected}"
            );
        }
        assert_eq!(Int8Query::new(&vector).unwrap().score(row).unwrap(), 1.);
        vector[dim - 1] = -1.;
        assert_eq!(Int8Query::new(&vector).unwrap().score(row).unwrap(), -1.);
    }
}

#[test]
fn integer_scores_match_independent_reconstructed_cosines() {
    for dim in [1, 65, 127, 128, 129, 384, 768, 769, 1536, 3072] {
        let (layout, bytes, reference) = reference_row(dim);
        let row = layout.row(&bytes).unwrap();
        let norm = reference
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        let mut decoded = vec![0f32; dim];
        row.decode_into(&mut decoded).unwrap();
        for (actual, expected) in decoded.iter().zip(&reference) {
            assert!((f64::from(*actual) - expected / norm).abs() < 1e-7);
        }
        // 这些查询的真实分量均为-1、0、1，量化后方向可以精确表示。
        for shift in 0..7 {
            let query = (0..dim)
                .map(|i| ((i + shift) % 3) as f32 - 1.)
                .collect::<Vec<_>>();
            let expected = cosine(
                &query.iter().map(|&x| f64::from(x)).collect::<Vec<_>>(),
                &reference,
            );
            let actual = Int8Query::new(&query).unwrap().score(row).unwrap();
            assert!(
                (f64::from(actual) - expected).abs() < 2e-7,
                "维度{dim} 查询{shift} 实际{actual} 预期{expected}"
            );
        }
    }
}

#[test]
fn query_rounding_and_sums_cover_signed_ties_and_partial_groups() {
    let input = [1f32, -1., 0.5, -0.5, 0., 0.25, -0.25];
    let query = Int8Query::new(&input).unwrap();
    assert_eq!(&query.codes[..7], &[127, -127, 64, -64, 0, 32, -32]);
    assert_eq!(query.group_sums[0], 0);
    assert!(query.codes[7..].iter().all(|&code| code == 0));
    let mut input = vec![1f32; 129];
    input[128] = -0.5;
    let query = Int8Query::new(&input).unwrap();
    assert_eq!(&query.group_sums[..2], &[16256, -64]);
}

#[test]
fn mismatched_decode_and_score_dimensions_are_structured_errors() {
    let layout = Int8Layout::from_header(&GOLDEN_HEADER).unwrap();
    let row = layout.row(&GOLDEN_ROW).unwrap();
    let mut output = [3f32; 3];
    assert!(matches!(
        row.decode_into(&mut output),
        Err(TriviumError::DimensionMismatch {
            expected: 4,
            got: 3
        })
    ));
    assert_eq!(output, [3f32; 3]);
    assert!(matches!(
        Int8Query::new(&[1f32; 3]).unwrap().score(row),
        Err(TriviumError::DimensionMismatch {
            expected: 3,
            got: 4
        })
    ));
}
