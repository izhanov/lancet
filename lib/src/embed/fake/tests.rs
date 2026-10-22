use super::*;

fn texts(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn deterministic() {
    let f = Fake { dim: 8 };
    let a = f.embed(&texts(&["hello", "world"])).await.unwrap();
    let b = f.embed(&texts(&["hello", "world"])).await.unwrap();
    assert_eq!(a, b);
}

#[tokio::test]
async fn distinct_texts() {
    let v = Fake { dim: 8 }
        .embed(&texts(&["hello", "world"]))
        .await
        .unwrap();
    assert_ne!(v[0], v[1], "different texts produced identical vectors");
}

#[tokio::test]
async fn dimension_and_norm() {
    let v = Fake { dim: 16 }.embed(&texts(&["hello"])).await.unwrap();
    assert_eq!(v[0].len(), 16);
    let norm: f32 = v[0].iter().map(|x| x * x).sum();
    assert!((norm - 1.0).abs() < 1e-3, "norm = {norm}, want ~1");
}

#[test]
fn id() {
    assert_eq!(Fake { dim: 32 }.id(), "fake/32");
}
