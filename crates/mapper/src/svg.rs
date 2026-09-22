//! Drawing a sheet as an SVG, for review outside the mapper.
//!
//! A plate submitted to `hydra-mapdb` is a claim about where rooms should
//! be drawn, and a reviewer should not have to build this program to see
//! it. So the same [`MapScene`] the canvas renders is written out as a
//! file a browser opens.
//!
//! **It is the canvas, not a second style.** The colours, the room size
//! and the line weights are the ones in [`crate::draw`], because two
//! renderers that are meant to agree will drift the moment they are
//! allowed to disagree. What differs is only what a file cannot have: no
//! camera, so the whole sheet is drawn at one scale; no hover or
//! selection, which are states of a pointer that is not there.

use std::fmt::Write as _;

use cena_map_layout::scene::{SceneEdgeKind, SheetScene};

/// Pixels per cell, and the room square inside one. The canvas's own
/// `CELL_PX` and `ROOM_PX` at scale 1.0 -- an SVG has no zoom, so the
/// unscaled size is the one to draw.
const CELL_PX: f32 = 28.0;
const ROOM_PX: f32 = 18.0;

/// A margin so the outermost rooms are not flush against the edge.
const MARGIN: f32 = CELL_PX;

/// The canvas's palette, as CSS. Kept in step with [`crate::draw`] by the
/// test below, which fails if either side is changed alone.
const CANVAS_BG: &str = "#181a1e";
const ROOM_FILL: &str = "#3c5a82";
const ROOM_STROKE: &str = "#8cb4dc";
const ENTRANCE_STROKE: &str = "#e6aa3c";
const ECHO_FILL: &str = "#463c23";
const DIRECTIONAL_LINE: &str = "#7896b4";
const CONNECTOR_LINE: &str = "#96785a";
const LABEL_COLOR: &str = "#dcdcc8";

/// How big a sheet may be before it is not worth writing.
///
/// An interiors shelf can run to thousands of rooms, and an SVG of those
/// is a file nobody opens in a browser twice. Refusing is more use than
/// emitting something unusable.
pub const MAX_ROOMS: usize = 1500;

/// Why a sheet was not drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotDrawn {
    /// Nothing on this sheet.
    Empty,
    /// More rooms than an SVG is any use for.
    TooBig(usize),
}

impl std::fmt::Display for NotDrawn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotDrawn::Empty => write!(f, "nothing on this sheet"),
            NotDrawn::TooBig(n) => {
                write!(f, "{n} rooms is past the {MAX_ROOMS} an SVG is useful at")
            }
        }
    }
}

/// Draw one sheet as a standalone SVG document.
///
/// `title` names the sheet inside the file, so one found on its own says
/// what it is.
///
/// # Errors
///
/// Returns [`NotDrawn`] when the sheet is empty or too large to be worth
/// drawing; neither is a failure, and both are worth telling a person.
///
/// Cell coordinates become pixel ones as `f32`. Losing precision there
/// would need a sheet 16 million cells across; the largest area on the
/// real map is a few hundred, and [`MAX_ROOMS`] caps it far below that.
#[allow(clippy::cast_precision_loss)]
pub fn sheet(scene: &SheetScene, title: &str) -> Result<String, NotDrawn> {
    if scene.rooms.is_empty() {
        return Err(NotDrawn::Empty);
    }
    if scene.rooms.len() > MAX_ROOMS {
        return Err(NotDrawn::TooBig(scene.rooms.len()));
    }

    // The sheet's own cell bounds, in pixels, with a margin.
    let cells_w = (scene.max.x - scene.min.x + 1) as f32;
    let cells_h = (scene.max.y - scene.min.y + 1) as f32;
    let width = cells_w.mul_add(CELL_PX, MARGIN * 2.0);
    let height = cells_h.mul_add(CELL_PX, MARGIN * 2.0);

    // Cell to pixel, centred in its cell like the canvas draws it.
    let px = |x: i32| MARGIN + (x - scene.min.x) as f32 * CELL_PX + CELL_PX / 2.0;
    let py = |y: i32| MARGIN + (y - scene.min.y) as f32 * CELL_PX + CELL_PX / 2.0;

    let mut svg = String::new();
    let _ = write!(
        svg,
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" "#,
            r#"viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}">"#,
            "\n<title>{title}</title>\n",
            r#"<rect width="100%" height="100%" fill="{bg}"/>"#,
            "\n"
        ),
        w = width,
        h = height,
        title = escape(title),
        bg = CANVAS_BG,
    );

    // Edges first, so rooms sit on top of them -- the canvas's own order.
    let _ = writeln!(svg, r#"<g stroke-linecap="round">"#);
    for edge in &scene.edges {
        let (color, w) = match edge.kind {
            SceneEdgeKind::Directional | SceneEdgeKind::Stub => (DIRECTIONAL_LINE, 1.5),
            SceneEdgeKind::Connector => (CONNECTOR_LINE, 1.05),
        };
        let _ = writeln!(
            svg,
            r#"<line x1="{:.1}" y1="{:.1}" x2="{:.1}" y2="{:.1}" stroke="{color}" stroke-width="{w}"/>"#,
            px(edge.a.x),
            py(edge.a.y),
            px(edge.b.x),
            py(edge.b.y),
        );
    }
    let _ = writeln!(svg, "</g>");

    let _ = writeln!(svg, r#"<g fill="{ROOM_FILL}">"#);
    for room in &scene.rooms {
        let stroke = if room.entrance {
            ENTRANCE_STROKE
        } else {
            ROOM_STROKE
        };
        // A room's number and title travel as a tooltip: an SVG has room
        // for what a canvas needs a hover to show.
        let _ = writeln!(
            svg,
            concat!(
                r#"<rect x="{x:.1}" y="{y:.1}" width="{side}" height="{side}" rx="2" "#,
                r#"stroke="{stroke}" stroke-width="1.5"><title>{tip}</title></rect>"#
            ),
            x = px(room.cell.x) - ROOM_PX / 2.0,
            y = py(room.cell.y) - ROOM_PX / 2.0,
            side = ROOM_PX,
            stroke = stroke,
            tip = escape(&format!("{} — {}", room.id.0, room.title)),
        );
    }
    let _ = writeln!(svg, "</g>");

    write_echoes(&mut svg, scene, &px, &py);

    if !scene.labels.is_empty() {
        let _ = writeln!(
            svg,
            r#"<g fill="{LABEL_COLOR}" font-family="sans-serif" font-size="12">"#
        );
        for label in &scene.labels {
            let _ = writeln!(
                svg,
                r#"<text x="{:.1}" y="{:.1}">{}</text>"#,
                px(label.cell.x) - ROOM_PX / 2.0,
                py(label.cell.y) - ROOM_PX / 2.0 - 2.0,
                escape(&label.text),
            );
        }
        let _ = writeln!(svg, "</g>");
    }

    let _ = writeln!(svg, "</svg>");
    Ok(svg)
}

/// Street rooms echoed among their buildings: signposts, drawn as the
/// canvas draws them, with the street's name beside each.
fn write_echoes(
    svg: &mut String,
    scene: &SheetScene,
    px: &dyn Fn(i32) -> f32,
    py: &dyn Fn(i32) -> f32,
) {
    if scene.anchors.is_empty() {
        return;
    }
    // Street rooms echoed among their buildings: signposts, drawn as the
    // canvas draws them, with the street's name beside each.
    let _ = writeln!(
        svg,
        r#"<g fill="{ECHO_FILL}" stroke="{ENTRANCE_STROKE}" stroke-width="2">"#
    );
    for echo in &scene.anchors {
        if !echo.building {
            let r = if echo.has_door { 5.4 } else { 3.2 };
            let _ = writeln!(
                svg,
                r#"<circle cx="{:.1}" cy="{:.1}" r="{r}" fill="{ENTRANCE_STROKE}" stroke="none"><title>{}</title></circle>"#,
                px(echo.cell.x),
                py(echo.cell.y),
                escape(&format!("{} — {} (street)", echo.id.0, echo.title)),
            );
            if echo.has_door && !echo.title.is_empty() {
                let _ = writeln!(
                    svg,
                    r#"<text x="{:.1}" y="{:.1}" fill="{ENTRANCE_STROKE}" stroke="none" font-family="sans-serif" font-size="11">{}</text>"#,
                    px(echo.cell.x) + r + 4.0,
                    py(echo.cell.y) + 4.0,
                    escape(&echo.title),
                );
            }
            continue;
        }
        let _ = writeln!(
            svg,
            concat!(
                r#"<rect x="{x:.1}" y="{y:.1}" width="{side}" height="{side}" rx="2">"#,
                r#"<title>{tip}</title></rect>"#,
                "
",
                r#"<text x="{tx:.1}" y="{ty:.1}" fill="{color}" stroke="none" "#,
                r#"font-family="sans-serif" font-size="11">{name}</text>"#
            ),
            x = px(echo.cell.x) - ROOM_PX / 2.0,
            y = py(echo.cell.y) - ROOM_PX / 2.0,
            side = ROOM_PX,
            tip = escape(&format!("{} — {}", echo.id.0, echo.title)),
            tx = px(echo.cell.x) + ROOM_PX / 2.0 + 4.0,
            ty = py(echo.cell.y) + 4.0,
            color = ENTRANCE_STROKE,
            name = escape(&echo.title),
        );
    }
    let _ = writeln!(svg, "</g>");
}

/// The five characters that cannot appear as text in XML.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cena_map::{Cost, Crossing, Exit, ExitKind, Map, Room, RoomId};
    use cena_map_layout::{build_scene, generate_layout};

    fn town() -> Map {
        let exit = |to: u32, command: &str| Exit {
            to: RoomId(to),
            kind: ExitKind::Cardinal,
            crossing: Crossing::Command(command.to_owned()),
            cost: Some(Cost::Fixed(1.0)),
        };
        let room = |id: u32, title: &str, exits: Vec<Exit>| Room {
            id: RoomId(id),
            uid: vec![],
            title: vec![title.to_owned()],
            description: vec![],
            paths: vec![],
            location: None,
            location_unknowable: false,
            check_location: false,
            unique_loot: vec![],
            climate: None,
            terrain: None,
            tags: vec![],
            meta: vec![],
            image: None,
            exits,
        };
        Map::from_rooms(vec![
            room(1, "[Town Square]", vec![exit(2, "east")]),
            room(2, "[Well & <Treehouse>]", vec![exit(1, "west")]),
        ])
        .expect("no duplicate ids")
    }

    fn scene_of(map: &Map) -> cena_map_layout::MapScene {
        build_scene("town", &generate_layout(map), map)
    }

    /// The file is well-formed SVG carrying every room and edge, at the
    /// size the sheet needs.
    #[test]
    fn a_sheet_draws_its_rooms_and_edges() {
        let map = town();
        let scene = scene_of(&map);
        let svg = sheet(&scene.outdoor, "town").expect("draws");

        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert_eq!(svg.matches("<rect").count(), 1 + scene.outdoor.rooms.len());
        assert_eq!(svg.matches("<line").count(), scene.outdoor.edges.len());
    }

    /// A room title is XML-escaped: titles carry `&` and angle brackets,
    /// and an unescaped one is a file no browser will open.
    #[test]
    fn markup_in_a_title_cannot_break_the_file() {
        let map = town();
        let svg = sheet(&scene_of(&map).outdoor, "town").expect("draws");
        assert!(
            svg.contains("Well &amp; &lt;Treehouse&gt;"),
            "a title was not escaped: {svg}"
        );
        assert!(
            !svg.contains("<Treehouse>"),
            "raw markup reached the file: {svg}"
        );
    }

    /// The document's own title is escaped too -- it is a plate name a
    /// person typed.
    #[test]
    fn a_plate_name_is_escaped_in_the_title() {
        let map = town();
        let svg = sheet(&scene_of(&map).outdoor, "Bob's <plate> & co").expect("draws");
        assert!(svg.contains("<title>Bob&apos;s &lt;plate&gt; &amp; co</title>"));
    }

    /// An empty sheet is reported rather than written as a blank file.
    #[test]
    fn an_empty_sheet_is_not_drawn() {
        let map = town();
        let scene = scene_of(&map);
        // This fixture is all outdoor, so its interiors shelf is empty.
        assert_eq!(sheet(&scene.interiors, "town"), Err(NotDrawn::Empty));
    }

    /// The SVG's palette is the canvas's. Two renderers meant to agree
    /// drift the moment one is changed alone, so this asserts they match
    /// rather than trusting a comment.
    #[test]
    fn the_palette_matches_the_canvas() {
        let hex = |c: egui::Color32| format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b());
        assert_eq!(hex(crate::draw::ROOM_FILL), ROOM_FILL);
        assert_eq!(hex(crate::draw::ROOM_STROKE), ROOM_STROKE);
        assert_eq!(hex(crate::draw::ENTRANCE_STROKE), ENTRANCE_STROKE);
        assert_eq!(hex(crate::draw::DIRECTIONAL_LINE), DIRECTIONAL_LINE);
        assert_eq!(hex(crate::draw::CONNECTOR_LINE), CONNECTOR_LINE);
        assert_eq!(hex(crate::draw::LABEL_COLOR), LABEL_COLOR);
        assert_eq!(hex(crate::draw::CANVAS_BG), CANVAS_BG);
        assert_eq!(hex(crate::draw::ECHO_FILL), ECHO_FILL);
    }
}
