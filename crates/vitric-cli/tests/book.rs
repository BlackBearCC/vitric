//! book 示例:2D 仿 3D 翻页——折叠/过脊/揭示/落定全链路 + 不可重入。

use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/book")
}

#[test]
fn flip_reveals_next_spread_and_lands() {
    let (mut sim, mut rt) = Runtime::boot(&dir()).unwrap();
    let leaf = sim.world.entity("leaf").unwrap();
    let tr = sim.world.entity("text-right").unwrap();
    let tl = sim.world.entity("text-left").unwrap();

    sim.inject_input("right", "pressed");
    sim.step(&mut rt).unwrap();
    assert_eq!(sim.world.get_field(leaf, "Leaf.flipping").unwrap(), &json!(true));
    // 折叠中:旧文字藏起,翻页中再按无效(不可重入)
    assert_eq!(sim.world.get_field(tr, "Text.content").unwrap(), &json!(""));
    sim.inject_input("right", "pressed");
    for _ in 0..40 {
        sim.step(&mut rt).unwrap();
    }
    // 落定:活页收起,左右页都是新内容,页码推进到下一跨页
    assert_eq!(sim.world.get_field(leaf, "Leaf.flipping").unwrap(), &json!(false));
    assert_eq!(sim.world.get_field(leaf, "Leaf.page").unwrap(), &json!(4));
    assert_eq!(sim.world.get_field(tr, "Text.content").unwrap(), &json!("PAGE 4"));
    assert_eq!(sim.world.get_field(tl, "Text.content").unwrap(), &json!("PAGE 3"));
    assert_eq!(sim.world.get_field(leaf, "Sprite.w").unwrap().as_f64(), Some(0.0));
}
