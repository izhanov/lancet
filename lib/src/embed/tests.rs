use super::*;

#[test]
fn normalize_scales_to_unit_length() {
    let mut vectors = vec![vec![3.0, 4.0], vec![0.0, 0.0]];
    normalize(&mut vectors);
    assert_eq!(vectors[0], vec![0.6, 0.8]);
    assert_eq!(vectors[1], vec![0.0, 0.0], "zero vector stays as is");
}
