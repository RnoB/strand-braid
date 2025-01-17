#[macro_use]
extern crate serde_derive;
extern crate bui_backend_types;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    pub theta: Option<f32>,
    pub area: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct ToClient {
    pub firehose_frame_data_url: String,
    pub found_points: Vec<Point>,
    pub valid_display: Option<Shape>,
    pub annotations: Vec<DrawableShape>,
    pub fno: u64,
    pub ts_rfc3339: String, // timestamp in RFC3339 format
    pub ck: bui_backend_types::ConnectionKey,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CircleParams {
    pub center_x: i16,
    pub center_y: i16,
    pub radius: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolygonParams {
    pub points: Vec<(f64, f64)>,
}

// #[derive(Debug,Clone, Serialize, Deserialize, PartialEq)]
// pub struct RectangleParams {
//     pub lower_x: i16,
//     pub lower_y: i16,
//     pub width: u16,
//     pub height: u16,
// }

// #[derive(Debug,Clone, Serialize, Deserialize, PartialEq)]
// pub struct MaskImage {
//     pub width: u16,
//     pub height: u16,
//     pub data: Vec<u8>,
// }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Shape {
    Everything,
    Circle(CircleParams),
    // Hole(CircleParams),
    // Rectangle(RectangleParams),
    // Mask(MaskImage),
    Polygon(PolygonParams),
}

// from client to server
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FirehoseCallbackInner {
    pub ck: bui_backend_types::ConnectionKey,
    pub fno: usize,
    pub ts_rfc3339: String,
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct RgbaColor {
    r: u8,
    g: u8,
    b: u8,
    a: f32,
}

impl From<RgbaColor> for String {
    fn from(orig: RgbaColor) -> String {
        format!("rgba({}, {}, {}, {:.2})", orig.r, orig.g, orig.b, orig.a)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum StrokeStyle {
    CssColor(RgbaColor),
    // CanvasGradient,
    // CanvasPattern,
}

impl StrokeStyle {
    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        StrokeStyle::CssColor(RgbaColor { r, g, b, a: 1.0 })
    }
}

impl From<StrokeStyle> for String {
    fn from(orig: StrokeStyle) -> String {
        match orig {
            StrokeStyle::CssColor(rgba) => rgba.into(),
        }
    }
}

/// A subset of the HTML5 canvas properties
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct DrawableShape {
    shape: Shape,
    stroke_style: StrokeStyle,
    line_width: f32,
}

impl DrawableShape {
    pub fn from_shape(shape: &Shape, stroke_style: &StrokeStyle, line_width: f32) -> Self {
        Self {
            shape: shape.clone(),
            stroke_style: stroke_style.clone(),
            line_width,
        }
    }
}

/// internal type for using in javascript. convert from `DrawlableShape`.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct CanvasDrawableShape {
    shape: Shape,
    stroke_style: String,
    line_width: f32,
}

impl From<DrawableShape> for CanvasDrawableShape {
    fn from(orig: DrawableShape) -> CanvasDrawableShape {
        CanvasDrawableShape {
            shape: orig.shape,
            stroke_style: orig.stroke_style.into(),
            line_width: orig.line_width,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::*;

    #[test]
    fn test_convert_drawable_shape() {
        let cps = CircleParams {
            center_x: 100,
            center_y: 200,
            radius: 50,
        };
        let shape = Shape::Circle(cps);
        let ss = StrokeStyle::from_rgb(1, 2, 3);
        let ds = DrawableShape::from_shape(&shape, &ss, 1.0);
        let cds: CanvasDrawableShape = ds.into();
        assert_eq!(cds.stroke_style, "rgba(1, 2, 3, 1.00)");
    }
}

pub const VIDEO_STREAM_EVENT_NAME: &'static str = "http-video-streaming";
