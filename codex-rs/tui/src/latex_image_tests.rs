use image::ImageEncoder;
use image::Rgba;
use image::RgbaImage;
use image::codecs::png::PngEncoder;
use pretty_assertions::assert_eq;
use ratex_types::display_item::DisplayList;

use super::*;

fn encode(image: &RgbaImage) -> image::ImageResult<Vec<u8>> {
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(bytes)
}

#[test]
fn formula_admission_trims_and_bounds_input() {
    let oversized = "x".repeat(MAX_FORMULA_BYTES + 1);

    assert_eq!(
        [
            ValidatedLatexFormula::new("  x^2  "),
            ValidatedLatexFormula::new("   "),
            ValidatedLatexFormula::new(&oversized),
            ValidatedLatexFormula::new(r"\text{naïve}"),
        ],
        [
            Ok(ValidatedLatexFormula("x^2".into())),
            Err(LatexImageError::InvalidFormula("formula is empty")),
            Err(LatexImageError::InvalidFormula("formula is too long")),
            Ok(ValidatedLatexFormula(r"\text{naïve}".into())),
        ],
    );
}

#[test]
fn formula_admission_rejects_custom_macro_definitions() {
    for control in [
        "def",
        "gdef",
        "edef",
        "xdef",
        "newcommand",
        "renewcommand",
        "providecommand",
        "global",
        "long",
        "let",
        "futurelet",
    ] {
        let formula = format!(r"\{control}\foo{{{}}}", "x".repeat(1024));
        assert_eq!(
            ValidatedLatexFormula::new(&formula),
            Err(LatexImageError::InvalidFormula(
                "custom macro definitions are not allowed"
            )),
            "control \\{control}",
        );
    }

    for formula in [
        r"\operatorname{definition}",
        r"\mathrm{renewcommand}",
        r"\\def",
        r"\λdef",
        r"\definitelyUnknown{x}",
    ] {
        assert_eq!(
            ValidatedLatexFormula::new(formula),
            Ok(ValidatedLatexFormula(formula.into())),
            "formula {formula}",
        );
    }

    let amplification = format!(r"\def\a{{{}}}{}", "x".repeat(8 * 1024), r"\a".repeat(900));
    assert!(amplification.len() < MAX_FORMULA_BYTES);
    assert_eq!(
        ValidatedLatexFormula::new(&amplification),
        Err(LatexImageError::InvalidFormula(
            "custom macro definitions are not allowed"
        )),
    );
}

#[test]
fn ratex_renders_a_transparent_colored_png() {
    let formula =
        ValidatedLatexFormula::new(r"\frac{-b \pm \sqrt{b^2-4ac}}{2a}").expect("valid formula");

    let png = render_formula(
        &formula,
        LatexRenderStyle::Display,
        /*cell_height*/ 16,
        [12, 34, 56],
    )
    .expect("render PNG");
    let rendered = image::load_from_memory_with_format(&png.bytes, ImageFormat::Png)
        .expect("decode rendered PNG")
        .into_rgba8();
    let strongest = rendered
        .pixels()
        .max_by_key(|pixel| pixel.0[3])
        .expect("rendered image has pixels");

    assert_eq!(
        (rendered.width(), rendered.height()),
        (png.width, png.height)
    );
    assert_eq!(rendered.get_pixel(/*x*/ 0, /*y*/ 0).0, [0, 0, 0, 0]);
    assert_eq!(&strongest.0[..3], &[12, 34, 56]);
    assert!(strongest.0[3] > 0);
}

#[test]
fn ratex_accepts_representative_display_math() {
    let formulas = [
        r"\begin{aligned}\rho\left(\frac{\partial \mathbf{u}}{\partial t}+(\mathbf{u}\cdot\nabla)\mathbf{u}\right)&=-\nabla p+\mu\nabla^2\mathbf{u}+\mathbf{f},\\\nabla\cdot\mathbf{u}&=0.\end{aligned}",
        r"\underbrace{\rho\frac{\partial\mathbf{u}}{\partial t}}_{\text{local acceleration}}",
        r"\begin{pmatrix}a&b\\c&d\end{pmatrix}^{-1}=\frac{1}{ad-bc}\begin{pmatrix}d&-b\\-c&a\end{pmatrix}",
        r"\begin{matrix}A&\xrightarrow{f}&B\\\downarrow g&&\downarrow h\\C&\xrightarrow{k}&D\end{matrix}",
        r"α+\beta=γ",
    ];

    for formula in formulas {
        let formula = ValidatedLatexFormula::new(formula).expect("valid formula");
        render_formula(
            &formula,
            LatexRenderStyle::Display,
            /*cell_height*/ 16,
            [255, 255, 255],
        )
        .unwrap_or_else(|error| panic!("failed to render {}: {error}", formula.as_str()));
    }
}

#[test]
fn ratex_uses_text_style_for_inline_math() {
    let formula = ValidatedLatexFormula::new(r"\sum_{i=1}^{n} i").expect("valid formula");

    let display = render_formula(
        &formula,
        LatexRenderStyle::Display,
        /*cell_height*/ 16,
        [255, 255, 255],
    )
    .expect("render display PNG");
    let inline = render_formula(
        &formula,
        LatexRenderStyle::Inline,
        /*cell_height*/ 16,
        [255, 255, 255],
    )
    .expect("render inline PNG");

    assert!(
        display.height > inline.height,
        "display dimensions {}x{} should be taller than inline dimensions {}x{}",
        display.width,
        display.height,
        inline.width,
        inline.height,
    );
}

#[test]
fn ratex_raster_dimensions_follow_terminal_cell_scale() {
    let formula = ValidatedLatexFormula::new(r"\frac{x}{y}").expect("valid formula");

    for style in [LatexRenderStyle::Display, LatexRenderStyle::Inline] {
        let reference = render_formula(&formula, style, /*cell_height*/ 32, [255, 255, 255])
            .expect("render reference PNG");
        let scaled = render_formula(&formula, style, /*cell_height*/ 48, [255, 255, 255])
            .expect("render scaled PNG");

        assert!(
            scaled.width > reference.width && scaled.height > reference.height,
            "{style:?} raster should grow from {}x{} to match the larger terminal cells, got {}x{}",
            reference.width,
            reference.height,
            scaled.width,
            scaled.height,
        );
    }
}

#[test]
fn ratex_parse_errors_fall_back_without_a_png() {
    let formula = ValidatedLatexFormula::new(r"\definitelyUnknown{x}").expect("bounded formula");

    assert!(matches!(
        render_formula(
            &formula,
            LatexRenderStyle::Display,
            /*cell_height*/ 16,
            [255, 255, 255],
        ),
        Err(LatexImageError::Render(_))
    ));
}

#[test]
fn renderer_rejects_system_font_fallback() {
    let formula = ValidatedLatexFormula::new(r#"\char"4E00"#).expect("bounded formula");

    assert!(matches!(
        render_formula(
            &formula,
            LatexRenderStyle::Display,
            /*cell_height*/ 16,
            [255, 255, 255],
        ),
        Err(LatexImageError::Render(reason))
            if reason == "formula requires a system font fallback"
    ));
}

#[test]
fn display_list_dimensions_are_checked_before_rasterization() {
    let dimensions = [
        (1.0, 1.0, 0.0),
        (f64::NAN, 1.0, 0.0),
        (100_000.0, 1.0, 0.0),
        (2_000.0, 2_000.0, 0.0),
    ];

    assert_eq!(
        dimensions.map(|(width, height, depth)| {
            validate_display_list_dimensions(
                &DisplayList {
                    items: Vec::new(),
                    width,
                    height,
                    depth,
                },
                raster_metrics(/*cell_height*/ 16),
            )
            .is_ok()
        }),
        [true, false, false, false],
    );
}

#[test]
fn png_validation_enforces_bytes_and_dimensions() {
    let visible = encode(&RgbaImage::from_pixel(
        /*width*/ 2,
        /*height*/ 2,
        Rgba([12, 34, 56, 255]),
    ))
    .expect("encode visible PNG fixture");
    let oversized_dimensions = encode(&RgbaImage::from_pixel(
        MAX_IMAGE_DIMENSION + 1,
        /*height*/ 1,
        Rgba([12, 34, 56, 255]),
    ))
    .expect("encode oversized PNG fixture");
    let oversized_bytes = vec![0; MAX_PNG_BYTES + 1];

    assert_eq!(
        validate_png(oversized_bytes),
        Err(LatexImageError::InvalidOutputSize(MAX_PNG_BYTES + 1)),
    );
    assert!(matches!(
        validate_png(oversized_dimensions),
        Err(LatexImageError::InvalidPng(reason)) if reason.contains("dimensions")
    ));
    assert_eq!(
        validate_png(visible).map(|png| (png.width, png.height)),
        Ok((2, 2)),
    );
}

#[test]
fn inline_strut_keeps_ordinary_formulas_at_one_scale() {
    let heights = ["x", r"\delta(x)", r"f'"].map(|source| {
        let formula = ValidatedLatexFormula::new(source).expect("valid formula");
        render_formula(
            &formula,
            LatexRenderStyle::Inline,
            /*cell_height*/ 16,
            [255, 255, 255],
        )
        .expect("render inline PNG")
        .height
    });

    assert_eq!(heights, [heights[0]; 3]);
}
