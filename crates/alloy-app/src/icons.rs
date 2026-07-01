use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    SkipBack,
    SkipForward,
    Square,
    Shuffle,
    Repeat,
    Repeat1,
    Search,
    Settings,
    Volume1,
    Volume2,
    VolumeX,
}

pub fn draw_icon(painter: &egui::Painter, rect: egui::Rect, icon: Icon, color: egui::Color32) {
    let size = rect.width().min(rect.height());
    let rect = egui::Rect::from_center_size(rect.center(), egui::vec2(size, size));
    let map = |x: f32, y: f32| -> egui::Pos2 {
        egui::pos2(
            rect.left() + rect.width() * x / 24.0,
            rect.top() + rect.height() * y / 24.0,
        )
    };
    let stroke = egui::Stroke::new((size / 24.0 * 2.0).clamp(1.4, 2.0), color);

    match icon {
        Icon::Play => polygon(
            painter,
            &[map(6.0, 3.0), map(20.0, 12.0), map(6.0, 21.0)],
            stroke,
        ),
        Icon::Pause => {
            line(painter, map(10.0, 4.0), map(10.0, 20.0), stroke);
            line(painter, map(14.0, 4.0), map(14.0, 20.0), stroke);
        }
        Icon::SkipBack => {
            line(painter, map(19.0, 20.0), map(9.0, 12.0), stroke);
            line(painter, map(9.0, 12.0), map(19.0, 4.0), stroke);
            line(painter, map(9.0, 20.0), map(9.0, 4.0), stroke);
            line(painter, map(5.0, 19.0), map(5.0, 5.0), stroke);
        }
        Icon::SkipForward => {
            line(painter, map(5.0, 4.0), map(15.0, 12.0), stroke);
            line(painter, map(15.0, 12.0), map(5.0, 20.0), stroke);
            line(painter, map(15.0, 4.0), map(15.0, 20.0), stroke);
            line(painter, map(19.0, 5.0), map(19.0, 19.0), stroke);
        }
        Icon::Square => {
            let rect = egui::Rect::from_min_max(map(6.0, 6.0), map(18.0, 18.0));
            painter.rect_stroke(
                rect,
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        Icon::Shuffle => {
            line(painter, map(2.0, 18.0), map(7.0, 18.0), stroke);
            line(painter, map(7.0, 18.0), map(17.0, 8.0), stroke);
            line(painter, map(17.0, 8.0), map(22.0, 8.0), stroke);
            line(painter, map(17.0, 4.0), map(22.0, 4.0), stroke);
            line(painter, map(18.0, 3.0), map(22.0, 4.0), stroke);
            line(painter, map(18.0, 5.0), map(22.0, 4.0), stroke);
            line(painter, map(2.0, 6.0), map(7.0, 6.0), stroke);
            line(painter, map(7.0, 6.0), map(17.0, 16.0), stroke);
            line(painter, map(17.0, 16.0), map(22.0, 16.0), stroke);
            line(painter, map(18.0, 15.0), map(22.0, 16.0), stroke);
            line(painter, map(18.0, 17.0), map(22.0, 16.0), stroke);
        }
        Icon::Repeat => {
            polyline(
                painter,
                &[map(17.0, 1.0), map(21.0, 5.0), map(17.0, 9.0)],
                stroke,
            );
            line(painter, map(3.0, 11.0), map(3.0, 9.0), stroke);
            line(painter, map(3.0, 9.0), map(3.0, 5.0), stroke);
            line(painter, map(3.0, 5.0), map(21.0, 5.0), stroke);
            polyline(
                painter,
                &[map(7.0, 23.0), map(3.0, 19.0), map(7.0, 15.0)],
                stroke,
            );
            line(painter, map(21.0, 13.0), map(21.0, 15.0), stroke);
            line(painter, map(21.0, 15.0), map(21.0, 19.0), stroke);
            line(painter, map(21.0, 19.0), map(3.0, 19.0), stroke);
        }
        Icon::Repeat1 => {
            draw_icon(painter, rect, Icon::Repeat, color);
            painter.text(
                map(12.0, 12.0),
                egui::Align2::CENTER_CENTER,
                "1",
                egui::FontId::proportional(size * 0.38),
                color,
            );
        }
        Icon::Search => {
            painter.circle_stroke(map(11.0, 11.0), size * 0.29, stroke);
            line(painter, map(21.0, 21.0), map(16.65, 16.65), stroke);
        }
        Icon::Settings => {
            painter.circle_stroke(map(12.0, 12.0), size * 0.18, stroke);
            for (a, b) in [
                ((12.0, 2.0), (12.0, 5.0)),
                ((12.0, 19.0), (12.0, 22.0)),
                ((4.93, 4.93), (7.05, 7.05)),
                ((16.95, 16.95), (19.07, 19.07)),
                ((2.0, 12.0), (5.0, 12.0)),
                ((19.0, 12.0), (22.0, 12.0)),
                ((4.93, 19.07), (7.05, 16.95)),
                ((16.95, 7.05), (19.07, 4.93)),
            ] {
                line(painter, map(a.0, a.1), map(b.0, b.1), stroke);
            }
        }
        Icon::Volume1 | Icon::Volume2 | Icon::VolumeX => {
            polygon(
                painter,
                &[
                    map(11.0, 5.0),
                    map(6.0, 9.0),
                    map(2.0, 9.0),
                    map(2.0, 15.0),
                    map(6.0, 15.0),
                    map(11.0, 19.0),
                ],
                stroke,
            );
            if matches!(icon, Icon::Volume1 | Icon::Volume2) {
                polyline(
                    painter,
                    &[map(15.54, 8.46), map(17.0, 12.0), map(15.54, 15.54)],
                    stroke,
                );
            }
            if icon == Icon::Volume2 {
                polyline(
                    painter,
                    &[map(19.07, 4.93), map(22.0, 12.0), map(19.07, 19.07)],
                    stroke,
                );
            }
            if icon == Icon::VolumeX {
                line(painter, map(22.0, 9.0), map(16.0, 15.0), stroke);
                line(painter, map(16.0, 9.0), map(22.0, 15.0), stroke);
            }
        }
    }
}

fn line(painter: &egui::Painter, a: egui::Pos2, b: egui::Pos2, stroke: egui::Stroke) {
    painter.line_segment([a, b], stroke);
}

fn polyline(painter: &egui::Painter, points: &[egui::Pos2], stroke: egui::Stroke) {
    painter.add(egui::Shape::line(points.to_vec(), stroke));
}

fn polygon(painter: &egui::Painter, points: &[egui::Pos2], stroke: egui::Stroke) {
    painter.add(egui::Shape::closed_line(points.to_vec(), stroke));
}
