use crate::renderer::vfx::spec::*;

#[derive(Clone, Debug)]
pub struct RibbonOpts {
    pub width: f32,
    pub cross_gradient: Gradient,
    pub length_gradient: Option<Gradient>,
    pub noise: Option<RibbonNoise>,
    pub blend: BlendMode,
}

pub fn ribbon(opts: RibbonOpts) -> Layer {
    Layer::Ribbon(RibbonSpec {
        width: opts.width,
        cross_gradient: opts.cross_gradient,
        length_gradient: opts.length_gradient,
        noise: opts.noise,
        blend: opts.blend,
    })
}

impl RibbonOpts {
    pub fn frost_outer() -> Self {
        Self {
            width: 0.45,
            cross_gradient: Gradient::from_stops([
                (0.00, [0.30, 0.60, 0.90, 0.0]),
                (0.20, [0.45, 0.85, 1.00, 0.6]),
                (0.50, [4.00, 6.00, 8.00, 1.0]),
                (0.80, [0.45, 0.85, 1.00, 0.6]),
                (1.00, [0.30, 0.60, 0.90, 0.0]),
            ]),
            length_gradient: Some(Gradient::from_stops([
                (0.00, [0.4, 0.4, 0.4, 0.6]),
                (0.10, [1.0, 1.0, 1.0, 1.0]),
                (0.85, [1.0, 1.0, 1.0, 1.0]),
                (1.00, [0.6, 0.6, 0.6, 0.4]),
            ])),
            noise: Some(RibbonNoise {
                tile: 0.5,
                scroll: 4.0,
                strength: 0.55,
                octaves: 3,
            }),
            blend: BlendMode::Additive,
        }
    }

    pub fn frost_inner() -> Self {
        Self {
            width: 0.14,
            cross_gradient: Gradient::from_stops([
                (0.00, [0.28, 0.70, 1.20, 0.0]),
                (0.34, [0.90, 2.20, 3.60, 0.46]),
                (0.50, [5.60, 8.20, 10.5, 1.00]),
                (0.66, [0.90, 2.20, 3.60, 0.46]),
                (1.00, [0.28, 0.70, 1.20, 0.0]),
            ]),
            length_gradient: Some(Gradient::from_stops([
                (0.00, [0.20, 0.26, 0.34, 0.0]),
                (0.12, [0.90, 1.05, 1.20, 0.90]),
                (0.74, [1.05, 1.18, 1.28, 1.00]),
                (1.00, [0.35, 0.50, 0.70, 0.0]),
            ])),
            noise: Some(RibbonNoise {
                tile: 0.20,
                scroll: 7.5,
                strength: 0.72,
                octaves: 4,
            }),
            blend: BlendMode::Additive,
        }
    }

    pub fn fire_outer() -> Self {
        Self {
            width: 0.45,
            cross_gradient: Gradient::from_stops([
                (0.00, [0.90, 0.35, 0.10, 0.0]),
                (0.20, [1.00, 0.55, 0.20, 0.6]),
                (0.50, [8.00, 4.00, 1.20, 1.0]),
                (0.80, [1.00, 0.55, 0.20, 0.6]),
                (1.00, [0.90, 0.35, 0.10, 0.0]),
            ]),
            length_gradient: Some(Gradient::from_stops([
                (0.00, [0.4, 0.4, 0.4, 0.6]),
                (0.10, [1.0, 1.0, 1.0, 1.0]),
                (0.85, [1.0, 1.0, 1.0, 1.0]),
                (1.00, [0.6, 0.6, 0.6, 0.4]),
            ])),
            noise: Some(RibbonNoise {
                tile: 0.5,
                scroll: 4.0,
                strength: 0.55,
                octaves: 3,
            }),
            blend: BlendMode::Additive,
        }
    }

    pub fn fire_inner() -> Self {
        Self {
            width: 0.16,
            cross_gradient: Gradient::from_stops([
                (0.00, [1.00, 0.45, 0.10, 0.0]),
                (0.36, [3.20, 1.30, 0.28, 0.48]),
                (0.50, [11.0, 5.60, 1.50, 1.00]),
                (0.64, [3.20, 1.30, 0.28, 0.48]),
                (1.00, [1.00, 0.45, 0.10, 0.0]),
            ]),
            length_gradient: Some(Gradient::from_stops([
                (0.00, [0.20, 0.20, 0.20, 0.0]),
                (0.08, [1.25, 1.10, 0.90, 0.85]),
                (0.70, [1.00, 0.92, 0.78, 1.00]),
                (1.00, [0.50, 0.30, 0.18, 0.0]),
            ])),
            noise: Some(RibbonNoise {
                tile: 0.22,
                scroll: 8.5,
                strength: 0.78,
                octaves: 4,
            }),
            blend: BlendMode::Additive,
        }
    }
}

pub fn frost_beam_ribbons() -> [Layer; 2] {
    [ribbon(RibbonOpts::frost_outer()), ribbon(RibbonOpts::frost_inner())]
}

pub fn fire_beam_ribbons() -> [Layer; 2] {
    [ribbon(RibbonOpts::fire_outer()), ribbon(RibbonOpts::fire_inner())]
}
