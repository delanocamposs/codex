use super::render;
use pretty_assertions::assert_eq;

#[test]
fn renders_single_line_math_with_common_glyphs() {
    assert_eq!(
        render(r"\rho(\mathbf{u}\cdot\nabla)\mathbf{u}"),
        Some("ρ(u·∇)u".to_string()),
    );
    assert_eq!(render(r"x\in\ker f''"), Some("x∈ker f″".to_string()),);
    assert_eq!(
        render(r"\delta(x),\quad x^2+y_{10},\quad \mathbb{R}"),
        Some("δ(x), x²+y₁₀, ℝ".to_string()),
    );
    assert_eq!(
        render(r"c^2=a^2+b^2-2ab\cos C,\quad \sin^2 x+\cos^2 x=1"),
        Some("c²=a²+b²-2ab cos C, sin² x+cos² x=1".to_string()),
    );
    assert_eq!(
        render(r"\sin(x),\quad \sin ^2(x),\quad 2\sin x"),
        Some("sin(x), sin²(x), 2 sin x".to_string()),
    );
    assert_eq!(
        render(r"K=\frac12mv^2,\quad x=\frac {3}{4}y"),
        Some("K=½mv², x=¾y".to_string()),
    );
}

#[test]
fn rejects_math_that_needs_layout_or_is_not_fully_understood() {
    for formula in [
        r"\frac{a}{b}",
        r"\frac{1}{11}",
        r"\sqrt{x}",
        r"\hat{x}",
        r"\sum_{i=0}^n",
        r"x_k",
        r"x\\y",
        r"\unknown{x}",
        r"{x",
    ] {
        assert_eq!(render(formula), None, "{formula}");
    }
}
