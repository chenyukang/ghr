use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::state::{DEFAULT_LIST_WIDTH_PERCENT, clamp_list_width_percent};

use super::AppState;

const CENTERED_RECT_MIN_WIDTH: u16 = 48;
const CENTERED_RECT_MAX_WIDTH: u16 = 112;

#[cfg(test)]
pub(super) fn body_area(area: Rect) -> Rect {
    page_areas(area)[2]
}

pub(super) fn details_area_for(app: &AppState, area: Rect) -> Rect {
    let chunks = page_areas(area);
    if app.mouse_capture_enabled {
        body_areas_with_ratio(chunks[2], app.list_width_percent)[1]
    } else {
        chunks[2]
    }
}

pub(super) fn page_areas(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(area)
}

#[cfg(test)]
pub(super) fn body_areas(area: Rect) -> std::rc::Rc<[Rect]> {
    body_areas_with_ratio(area, DEFAULT_LIST_WIDTH_PERCENT)
}

pub(super) fn body_areas_with_ratio(area: Rect, list_width_percent: u16) -> std::rc::Rc<[Rect]> {
    let list_width_percent = clamp_list_width_percent(list_width_percent);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(list_width_percent),
            Constraint::Percentage(100 - list_width_percent),
        ])
        .split(area)
}

pub(super) fn block_inner(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    }
}

pub(super) fn rect_contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x
        && x < area.x.saturating_add(area.width)
        && y >= area.y
        && y < area.y.saturating_add(area.height)
}

pub(super) fn splitter_contains(body: Rect, list: Rect, details: Rect, x: u16, y: u16) -> bool {
    if !rect_contains(body, x, y) {
        return false;
    }

    let list_border = list.x.saturating_add(list.width).saturating_sub(1);
    x == list_border || x == details.x
}

pub(super) fn split_percent_from_column(body: Rect, column: u16) -> u16 {
    if body.width == 0 {
        return DEFAULT_LIST_WIDTH_PERCENT;
    }

    let left_width = column.saturating_sub(body.x).min(body.width);
    let percent = (u32::from(left_width) * 100 + u32::from(body.width) / 2) / u32::from(body.width);
    clamp_list_width_percent(percent as u16)
}

pub(super) fn centered_rect(width_percent: u16, height: u16, area: Rect) -> Rect {
    let width = centered_rect_width(width_percent, area);
    let height = height.min(area.height);
    centered_rect_with_size(width, height, area)
}

pub(super) fn centered_rect_width(width_percent: u16, area: Rect) -> u16 {
    let mut width = area.width.saturating_mul(width_percent).saturating_div(100);
    width = width
        .max(CENTERED_RECT_MIN_WIDTH.min(area.width))
        .min(CENTERED_RECT_MAX_WIDTH.min(area.width));
    width
}

pub(super) fn centered_rect_with_size(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_rect_width_caps_wide_terminals() {
        let area = Rect::new(0, 0, 240, 40);

        assert_eq!(centered_rect_width(78, area), CENTERED_RECT_MAX_WIDTH);
        assert_eq!(centered_rect(78, 14, area), Rect::new(64, 13, 112, 14));
    }

    #[test]
    fn centered_rect_width_keeps_percentage_before_cap() {
        let area = Rect::new(0, 0, 120, 40);

        assert_eq!(centered_rect_width(76, area), 91);
    }
}
