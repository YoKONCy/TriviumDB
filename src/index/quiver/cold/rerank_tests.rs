//! 固定方向、独立编码字节和独立余弦预期覆盖量化与评分契约。
use super::*;

#[test]
fn all_dimensions_encodings_and_query_precisions_keep_direction_and_zero_semantics() {
    for dimension in 1..=MAX_BQ_DIM {
        let mut vector = vec![0f32; dimension];
        vector[dimension - 1] = 1.;
        for encoding in [
            RerankEncoding::Int8,
            RerankEncoding::CenteredInt8,
            RerankEncoding::CenteredInt4,
        ] {
            let layout = ColdLayout::new(dimension, 64, encoding).unwrap();
            let mut center = vec![0.; layout.center_bytes() / 4];
            if encoding.is_centered() {
                center[dimension - 1] = 0.25;
            }
            let mut bytes = vec![0; layout.row_stride_bytes()];
            layout.encode(&vector, &center, &mut bytes).unwrap();
            let restored = ColdLayout::from_header(&layout.to_header()).unwrap();
            assert_eq!(restored, layout);
            let row = layout.row(&bytes, &center).unwrap();
            for precision in [
                QueryPrecision::Fp32,
                QueryPrecision::Int8,
                QueryPrecision::Int4,
            ] {
                let query = RerankQuery::new(&vector, precision, &center).unwrap();
                assert!(
                    (query.score(row).unwrap() - 1.).abs() < 1e-6,
                    "维度{dimension}，{encoding:?}/{precision:?}"
                );
                let zero = RerankQuery::new(&vec![0.; dimension], precision, &center).unwrap();
                assert_eq!(zero.score(row).unwrap(), 0.);
            }
            layout
                .encode(&vec![0.; dimension], &center, &mut bytes)
                .unwrap();
            let zero = layout.row(&bytes, &center).unwrap();
            let mut decoded = vec![1.; dimension];
            zero.decode_into(&mut decoded).unwrap();
            assert_eq!(decoded, vec![0.; dimension]);
            assert_eq!(
                RerankQuery::new(&vector, QueryPrecision::Fp32, &center)
                    .unwrap()
                    .score(zero)
                    .unwrap(),
                0.
            );
        }
    }
}

#[test]
fn packed_nibbles_parameters_and_centered_reconstruction_match_independent_values() {
    let layout = ColdLayout::new(3, 1, RerankEncoding::CenteredInt4).unwrap();
    let center = [0.25, 0.25, 0.];
    let mut bytes = vec![0; layout.row_stride_bytes()];
    layout.encode(&[1., 0., 0.], &center, &mut bytes).unwrap();
    // 残差为[0.75,-0.25,0]，量化码为[15,0,4]，末字节高半字节为零。
    assert_eq!(&bytes[..2], &[0x0f, 0x04]);
    assert_eq!(read_f32(&bytes, 2), (1f64 / 15.) as f32);
    assert_eq!(read_f32(&bytes, 6), -0.25);
    let scale = f64::from((1f64 / 15.) as f32);
    let x = 15. * scale;
    let z = 4. * scale - 0.25;
    let inverse = (1. / (x * x + z * z).sqrt()) as f32;
    let row = layout.row(&bytes, &center).unwrap();
    let query = RerankQuery::new(&[1., 0., 0.], QueryPrecision::Fp32, &center).unwrap();
    assert_eq!(query.score(row).unwrap(), (x * f64::from(inverse)) as f32);
    let mut decoded = [0.; 3];
    row.decode_into(&mut decoded).unwrap();
    assert_eq!(
        decoded,
        [
            (x * f64::from(inverse)) as f32,
            0.,
            (z * f64::from(inverse)) as f32
        ]
    );
}

#[test]
fn query_precision_uses_its_own_center_dot_and_norm() {
    for encoding in [
        RerankEncoding::Int8,
        RerankEncoding::CenteredInt8,
        RerankEncoding::CenteredInt4,
    ] {
        let layout = ColdLayout::new(2, 1, encoding).unwrap();
        let center = if encoding.is_centered() {
            vec![0.25, 0.]
        } else {
            Vec::new()
        };
        let mut bytes = vec![0; layout.row_stride_bytes()];
        layout.encode(&[1., 0.], &center, &mut bytes).unwrap();
        let row = layout.row(&bytes, &center).unwrap();
        for (precision, expected) in [
            (
                QueryPrecision::Fp32,
                1. / (1. + f64::from(0.1f32).powi(2)).sqrt(),
            ),
            (
                QueryPrecision::Int8,
                127. / (127f64.powi(2) + 13f64.powi(2)).sqrt(),
            ),
            (QueryPrecision::Int4, 7. / 50f64.sqrt()),
        ] {
            let score = RerankQuery::new(&[1., 0.1], precision, &center)
                .unwrap()
                .score(row)
                .unwrap();
            assert!(
                (f64::from(score) - expected).abs() < 2e-7,
                "{encoding:?}/{precision:?}: {score}"
            );
        }
    }
}

#[test]
fn int4_encoding_rounds_exact_half_levels_to_even_codes() {
    let layout = ColdLayout::new(5, 1, RerankEncoding::CenteredInt4).unwrap();
    let center = [0.0625, 0., -0.03125, -0.09375, -0.15625];
    let mut bytes = vec![0; layout.row_stride_bytes()];
    layout
        .encode(&[1., 0., 0., 0., 0.], &center, &mut bytes)
        .unwrap();
    // scale为1/16，三个中间分量位于0.5、1.5、2.5格，偶数舍入分别为0、2、2。
    assert_eq!(&bytes[..3], &[0x0f, 0x20, 0x02]);
    assert_eq!(read_f32(&bytes, 3), 0.0625);
    layout.row(&bytes, &center).unwrap();
}

#[test]
fn new_layouts_reject_truncation_corruption_invalid_centers_and_tail_padding() {
    for encoding in [RerankEncoding::CenteredInt8, RerankEncoding::CenteredInt4] {
        let layout = ColdLayout::new(129, 64, encoding).unwrap();
        let center = vec![0.; 129];
        let mut bytes = vec![0; layout.row_stride_bytes()];
        layout.encode(&vec![1.; 129], &center, &mut bytes).unwrap();
        for i in 0..bytes.len() {
            assert!(layout.row(&bytes[..i], &center).is_err());
            let mut damaged = bytes.clone();
            damaged[i] ^= 1;
            assert!(layout.row(&damaged, &center).is_err(), "损坏偏移{i}");
        }
        assert!(layout.row(&bytes, &[0.; 128]).is_err());
        let mut bad_center = center.clone();
        bad_center[0] = f32::NAN;
        assert!(
            layout
                .encode(&vec![1.; 129], &bad_center, &mut bytes)
                .is_err()
        );
        if encoding == RerankEncoding::CenteredInt4 {
            bytes[64] |= 0x80;
            let crc = crc32fast::hash(&bytes[..layout.payload_bytes() - 4]);
            bytes[layout.payload_bytes() - 4..layout.payload_bytes()]
                .copy_from_slice(&crc.to_le_bytes());
            assert!(layout.row(&bytes, &center).is_err());
        }
    }
}

#[test]
fn simd_and_tail_kernels_match_independent_integer_dot_products() {
    for bits in [4, 8] {
        for length in [1usize, 3, 127, 128] {
            let query = (0..length)
                .map(|i| (i as i32 % 255 - 127) as i8)
                .collect::<Vec<_>>();
            let values = (0..length)
                .map(|i| {
                    if bits == 4 {
                        (i % 16) as i32
                    } else {
                        i as i32 % 255 - 127
                    }
                })
                .collect::<Vec<_>>();
            let mut codes = vec![
                0;
                if bits == 4 {
                    length.div_ceil(2)
                } else {
                    length
                }
            ];
            for (i, &value) in values.iter().enumerate() {
                if bits == 4 {
                    codes[i / 2] |= (value as u8) << ((i % 2) * 4);
                } else {
                    codes[i] = value as i8 as u8;
                }
            }
            let expected = query
                .iter()
                .zip(&values)
                .map(|(&q, &x)| i64::from(q) * i64::from(x))
                .sum::<i64>();
            assert_eq!(i64::from(kernels::integer(&query, &codes, bits)), expected);
            let floats = query
                .iter()
                .map(|&x| f64::from(x) * 0.125)
                .collect::<Vec<_>>();
            assert_eq!(
                kernels::floating(&floats, &codes, bits),
                expected as f64 * 0.125
            );
        }
    }
}

#[test]
fn corrected_checksums_cannot_turn_a_nonzero_row_into_a_zero_factor_row() {
    for encoding in [RerankEncoding::CenteredInt8, RerankEncoding::CenteredInt4] {
        let layout = ColdLayout::new(3, 64, encoding).unwrap();
        let mut bytes = vec![0; layout.row_stride_bytes()];
        let center = [0.25, 0., 0.];
        layout.encode(&[1., 0., 0.], &center, &mut bytes).unwrap();
        let offset = layout.payload_bytes() - 8;
        bytes[offset..offset + 4].copy_from_slice(&0f32.to_le_bytes());
        let crc = crc32fast::hash(&bytes[..layout.payload_bytes() - 4]);
        bytes[layout.payload_bytes() - 4..layout.payload_bytes()]
            .copy_from_slice(&crc.to_le_bytes());
        assert_eq!(
            layout.row(&bytes, &center).err().unwrap().code(),
            "TDB_CORRUPTED_FILE"
        );
    }
}

#[test]
fn centered_zero_vectors_decode_and_score_as_positive_zero() {
    for encoding in [RerankEncoding::CenteredInt8, RerankEncoding::CenteredInt4] {
        let layout = ColdLayout::new(3, 64, encoding).unwrap();
        let center = [0.25, 0.125, 0.33];
        let mut bytes = vec![0; layout.row_stride_bytes()];
        layout.encode(&[0.; 3], &center, &mut bytes).unwrap();
        let row = layout.row(&bytes, &center).unwrap();
        let mut decoded = [1.; 3];
        row.decode_into(&mut decoded).unwrap();
        assert_eq!(decoded.map(f32::to_bits), [0; 3]);
        for precision in [
            QueryPrecision::Fp32,
            QueryPrecision::Int8,
            QueryPrecision::Int4,
        ] {
            for query in [
                [1., 0., 0.],
                [-1., 0., 0.],
                [0., 1., 0.],
                [0., -1., 0.],
                [0., 0., 1.],
                [0., 0., -1.],
            ] {
                assert_eq!(
                    RerankQuery::new(&query, precision, &center)
                        .unwrap()
                        .score(row)
                        .unwrap()
                        .to_bits(),
                    0
                );
            }
        }
    }
}
