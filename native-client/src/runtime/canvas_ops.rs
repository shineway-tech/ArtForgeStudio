use super::*;

const MAX_CANVAS_HISTORY: usize = 100;
pub(super) const GROUP_PADDING: f32 = 36.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct CanvasRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl CanvasRect {
    pub fn normalized(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self {
            x: x1.min(x2),
            y: y1.min(y2),
            width: (x2 - x1).abs(),
            height: (y2 - y1).abs(),
        }
    }

    fn intersects(self, other: Self) -> bool {
        self.x <= other.x + other.width
            && self.x + self.width >= other.x
            && self.y <= other.y + other.height
            && self.y + self.height >= other.y
    }

    fn contains_point(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }

    fn area(self) -> f32 {
        self.width.max(0.0) * self.height.max(0.0)
    }
}

fn note_rect(note: &CanvasNoteData) -> CanvasRect {
    CanvasRect {
        x: note.x,
        y: note.y,
        width: note.width,
        height: note.height,
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct CanvasSnapshot {
    pub notes: Vec<CanvasNoteData>,
    pub links: Vec<CanvasLinkData>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct CanvasClipboard {
    pub notes: Vec<CanvasNoteData>,
    pub links: Vec<CanvasLinkData>,
}

#[derive(Default)]
pub(super) struct CanvasController {
    undo: Vec<CanvasSnapshot>,
    redo: Vec<CanvasSnapshot>,
    pub clipboard: CanvasClipboard,
}

impl CanvasController {
    pub fn record(&mut self, snapshot: CanvasSnapshot) {
        self.undo.push(snapshot);
        if self.undo.len() > MAX_CANVAS_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, current: CanvasSnapshot) -> Option<CanvasSnapshot> {
        let previous = self.undo.pop()?;
        self.redo.push(current);
        Some(previous)
    }

    pub fn redo(&mut self, current: CanvasSnapshot) -> Option<CanvasSnapshot> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn copy_selection(&mut self, notes: &[CanvasNoteData], links: &[CanvasLinkData]) {
        let ids = expanded_selection_ids(notes);
        self.clipboard.notes = notes
            .iter()
            .filter(|note| ids.contains(&note.id))
            .cloned()
            .collect();
        self.clipboard.links = links
            .iter()
            .filter(|link| ids.contains(&link.source_id) && ids.contains(&link.target_id))
            .cloned()
            .collect();
    }

    pub fn paste_clipboard(
        &self,
        offset_x: f32,
        offset_y: f32,
    ) -> (Vec<CanvasNoteData>, Vec<CanvasLinkData>) {
        let id_map = self
            .clipboard
            .notes
            .iter()
            .map(|note| (note.id.clone(), Uuid::new_v4().to_string()))
            .collect::<BTreeMap<_, _>>();
        let notes = self
            .clipboard
            .notes
            .iter()
            .cloned()
            .map(|mut note| {
                note.id = id_map.get(&note.id).cloned().unwrap_or_default();
                note.parent_group_id = id_map
                    .get(&note.parent_group_id)
                    .cloned()
                    .unwrap_or_default();
                note.x += offset_x;
                note.y += offset_y;
                note.selected = true;
                note
            })
            .collect();
        let links = self
            .clipboard
            .links
            .iter()
            .filter_map(|link| {
                Some(CanvasLinkData {
                    id: Uuid::new_v4().to_string(),
                    source_id: id_map.get(&link.source_id)?.clone(),
                    target_id: id_map.get(&link.target_id)?.clone(),
                })
            })
            .collect();
        (notes, links)
    }
}

pub(super) fn canvas_snapshot(store: &Store) -> CanvasSnapshot {
    CanvasSnapshot {
        notes: store.canvas_notes.clone(),
        links: store.canvas_links.clone(),
    }
}

pub(super) fn restore_canvas_snapshot(store: &mut Store, snapshot: CanvasSnapshot) {
    store.canvas_notes = snapshot.notes;
    store.canvas_links = snapshot.links;
}

pub(super) fn selected_ids(notes: &[CanvasNoteData]) -> BTreeSet<String> {
    notes
        .iter()
        .filter(|note| note.selected)
        .map(|note| note.id.clone())
        .collect()
}

pub(super) fn descendant_ids(notes: &[CanvasNoteData], group_id: &str) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let mut pending = vec![group_id.to_string()];
    while let Some(parent_id) = pending.pop() {
        for child in notes
            .iter()
            .filter(|note| note.parent_group_id == parent_id)
        {
            if result.insert(child.id.clone()) && child.kind == "group" {
                pending.push(child.id.clone());
            }
        }
    }
    result
}

pub(super) fn expanded_selection_ids(notes: &[CanvasNoteData]) -> BTreeSet<String> {
    let mut ids = selected_ids(notes);
    let selected_groups = notes
        .iter()
        .filter(|note| note.selected && note.kind == "group")
        .map(|note| note.id.clone())
        .collect::<Vec<_>>();
    for group_id in selected_groups {
        ids.extend(descendant_ids(notes, &group_id));
    }
    ids
}

pub(super) fn move_selection(notes: &mut [CanvasNoteData], dx: f32, dy: f32) {
    let ids = expanded_selection_ids(notes);
    for note in notes.iter_mut().filter(|note| ids.contains(&note.id)) {
        note.x += dx;
        note.y += dy;
    }
}

pub(super) fn clear_selection(notes: &mut [CanvasNoteData]) {
    for note in notes {
        note.selected = false;
    }
}

pub(super) fn select_node(notes: &mut [CanvasNoteData], id: &str, toggle: bool) {
    if !toggle {
        clear_selection(notes);
    }
    if let Some(note) = notes.iter_mut().find(|note| note.id == id) {
        note.selected = if toggle { !note.selected } else { true };
    }
}

pub(super) fn select_in_rect(notes: &mut [CanvasNoteData], rect: CanvasRect, additive: bool) {
    if !additive {
        clear_selection(notes);
    }
    for note in notes {
        if rect.intersects(note_rect(note)) {
            note.selected = true;
        }
    }
}

pub(super) fn selection_bounds(
    notes: &[CanvasNoteData],
    ids: &BTreeSet<String>,
) -> Option<CanvasRect> {
    let mut selected = notes.iter().filter(|note| ids.contains(&note.id));
    let first = selected.next()?;
    let mut left = first.x;
    let mut top = first.y;
    let mut right = first.x + first.width;
    let mut bottom = first.y + first.height;
    for note in selected {
        left = left.min(note.x);
        top = top.min(note.y);
        right = right.max(note.x + note.width);
        bottom = bottom.max(note.y + note.height);
    }
    Some(CanvasRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

pub(super) fn group_depth(notes: &[CanvasNoteData], group_id: &str) -> usize {
    let parents = notes
        .iter()
        .map(|note| (note.id.as_str(), note.parent_group_id.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut depth = 0;
    let mut current = group_id;
    let mut visited = BTreeSet::new();
    while let Some(parent) = parents.get(current).copied() {
        if parent.is_empty() || !visited.insert(parent) {
            break;
        }
        depth += 1;
        current = parent;
    }
    depth
}

pub(super) fn would_create_group_cycle(
    notes: &[CanvasNoteData],
    child_id: &str,
    parent_id: &str,
) -> bool {
    child_id == parent_id || descendant_ids(notes, child_id).contains(parent_id)
}

pub(super) fn deepest_containing_group(
    notes: &[CanvasNoteData],
    node: &CanvasNoteData,
    excluded_ids: &BTreeSet<String>,
) -> Option<String> {
    let center_x = node.x + node.width / 2.0;
    let center_y = node.y + node.height / 2.0;
    notes
        .iter()
        .filter(|group| {
            group.kind == "group"
                && !excluded_ids.contains(&group.id)
                && note_rect(group).contains_point(center_x, center_y)
                && !would_create_group_cycle(notes, &node.id, &group.id)
        })
        .min_by(|left, right| {
            let left_depth = group_depth(notes, &left.id);
            let right_depth = group_depth(notes, &right.id);
            right_depth
                .cmp(&left_depth)
                .then_with(|| note_rect(left).area().total_cmp(&note_rect(right).area()))
        })
        .map(|group| group.id.clone())
}

pub(super) fn assign_deepest_group(notes: &mut [CanvasNoteData], moved_ids: &BTreeSet<String>) {
    let assignments = notes
        .iter()
        .filter(|note| moved_ids.contains(&note.id))
        .filter(|note| {
            note.parent_group_id.is_empty() || !moved_ids.contains(&note.parent_group_id)
        })
        .map(|note| {
            (
                note.id.clone(),
                deepest_containing_group(notes, note, moved_ids).unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    for (id, parent_id) in assignments {
        if let Some(note) = notes.iter_mut().find(|note| note.id == id) {
            note.parent_group_id = parent_id;
        }
    }
}

pub(super) fn group_selection(notes: &mut Vec<CanvasNoteData>, english: bool) -> Option<String> {
    let ids = expanded_selection_ids(notes);
    let bounds = selection_bounds(notes, &ids)?;
    let id = Uuid::new_v4().to_string();
    let parent_group_id = notes
        .iter()
        .find(|note| ids.contains(&note.id))
        .map(|note| note.parent_group_id.clone())
        .unwrap_or_default();
    for note in notes.iter_mut().filter(|note| ids.contains(&note.id)) {
        if note.parent_group_id == parent_group_id {
            note.parent_group_id = id.clone();
            note.selected = false;
        }
    }
    notes.push(CanvasNoteData {
        id: id.clone(),
        kind: "group".into(),
        content: if english { "Group" } else { "分组" }.into(),
        x: bounds.x - GROUP_PADDING,
        y: bounds.y - GROUP_PADDING,
        width: bounds.width + GROUP_PADDING * 2.0,
        height: bounds.height + GROUP_PADDING * 2.0,
        parent_group_id,
        selected: true,
        ..CanvasNoteData::default()
    });
    Some(id)
}

pub(super) fn ungroup_selection(notes: &mut Vec<CanvasNoteData>) -> BTreeSet<String> {
    let selected_groups = notes
        .iter()
        .filter(|note| note.selected && note.kind == "group")
        .map(|note| (note.id.clone(), note.parent_group_id.clone()))
        .collect::<Vec<_>>();
    let removed = selected_groups
        .iter()
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    for (group_id, parent_id) in &selected_groups {
        for child in notes
            .iter_mut()
            .filter(|note| note.parent_group_id == *group_id)
        {
            child.parent_group_id = parent_id.clone();
            child.selected = true;
        }
    }
    notes.retain(|note| !removed.contains(&note.id));
    removed
}

pub(super) fn resize_group(
    notes: &mut [CanvasNoteData],
    group_id: &str,
    requested_width: f32,
    requested_height: f32,
) -> bool {
    let Some(group) = notes
        .iter()
        .find(|note| note.id == group_id && note.kind == "group")
    else {
        return false;
    };
    let group_x = group.x;
    let group_y = group.y;
    let descendants = descendant_ids(notes, group_id);
    let required_width = notes
        .iter()
        .filter(|note| descendants.contains(&note.id))
        .map(|note| note.x + note.width - group_x + GROUP_PADDING)
        .fold(160.0_f32, f32::max);
    let required_height = notes
        .iter()
        .filter(|note| descendants.contains(&note.id))
        .map(|note| note.y + note.height - group_y + GROUP_PADDING)
        .fold(120.0_f32, f32::max);
    let Some(group) = notes.iter_mut().find(|note| note.id == group_id) else {
        return false;
    };
    let width = requested_width.max(required_width);
    let height = requested_height.max(required_height);
    if group.width == width && group.height == height {
        return false;
    }
    group.width = width;
    group.height = height;
    true
}

pub(super) fn remove_selection(
    notes: &mut Vec<CanvasNoteData>,
    links: &mut Vec<CanvasLinkData>,
) -> BTreeSet<String> {
    let removed = selected_ids(notes);
    let group_parents = notes
        .iter()
        .filter(|note| removed.contains(&note.id) && note.kind == "group")
        .map(|note| (note.id.clone(), note.parent_group_id.clone()))
        .collect::<BTreeMap<_, _>>();
    for note in notes.iter_mut() {
        if let Some(parent_id) = group_parents.get(&note.parent_group_id) {
            note.parent_group_id = parent_id.clone();
        }
    }
    notes.retain(|note| !removed.contains(&note.id));
    links.retain(|link| !removed.contains(&link.source_id) && !removed.contains(&link.target_id));
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, kind: &str, x: f32, y: f32) -> CanvasNoteData {
        CanvasNoteData {
            id: id.into(),
            kind: kind.into(),
            x,
            y,
            width: 100.0,
            height: 80.0,
            ..CanvasNoteData::default()
        }
    }

    #[test]
    fn canvas_ops_selected_descendants_move_once_with_their_group() {
        let mut notes = vec![
            CanvasNoteData {
                selected: true,
                ..note("group", "group", 0.0, 0.0)
            },
            CanvasNoteData {
                parent_group_id: "group".into(),
                selected: true,
                ..note("child", "text", 20.0, 20.0)
            },
        ];

        move_selection(&mut notes, 10.0, 15.0);

        assert_eq!((notes[0].x, notes[0].y), (10.0, 15.0));
        assert_eq!((notes[1].x, notes[1].y), (30.0, 35.0));
    }

    #[test]
    fn canvas_ops_grouping_uses_the_deepest_valid_container() {
        let mut notes = vec![
            CanvasNoteData {
                width: 500.0,
                height: 500.0,
                ..note("outer", "group", 0.0, 0.0)
            },
            CanvasNoteData {
                parent_group_id: "outer".into(),
                width: 250.0,
                height: 250.0,
                ..note("inner", "group", 50.0, 50.0)
            },
            CanvasNoteData {
                selected: true,
                ..note("node", "text", 100.0, 100.0)
            },
        ];
        let moved = selected_ids(&notes);

        assign_deepest_group(&mut notes, &moved);

        assert_eq!(notes[2].parent_group_id, "inner");
    }

    #[test]
    fn canvas_ops_group_cannot_be_parented_to_its_descendant() {
        let notes = vec![
            note("outer", "group", 0.0, 0.0),
            CanvasNoteData {
                parent_group_id: "outer".into(),
                ..note("inner", "group", 20.0, 20.0)
            },
        ];

        assert!(would_create_group_cycle(&notes, "outer", "inner"));
        assert!(!would_create_group_cycle(&notes, "inner", "outer"));
    }

    #[test]
    fn canvas_ops_one_drag_creates_one_history_entry() {
        let mut controller = CanvasController::default();
        let before = CanvasSnapshot {
            notes: vec![note("node", "text", 0.0, 0.0)],
            links: Vec::new(),
        };
        controller.record(before.clone());

        let current = CanvasSnapshot {
            notes: vec![note("node", "text", 40.0, 30.0)],
            links: Vec::new(),
        };
        assert_eq!(controller.undo(current.clone()), Some(before));
        assert_eq!(controller.redo(CanvasSnapshot::default()), Some(current));
    }

    #[test]
    fn canvas_clipboard_copies_internal_links_and_remaps_group_ids() {
        let notes = vec![
            CanvasNoteData {
                selected: true,
                ..note("group", "group", 0.0, 0.0)
            },
            CanvasNoteData {
                parent_group_id: "group".into(),
                ..note("a", "text", 20.0, 20.0)
            },
            CanvasNoteData {
                parent_group_id: "group".into(),
                ..note("b", "text", 160.0, 20.0)
            },
            note("outside", "text", 500.0, 20.0),
        ];
        let links = vec![
            CanvasLinkData {
                id: "internal".into(),
                source_id: "a".into(),
                target_id: "b".into(),
            },
            CanvasLinkData {
                id: "external".into(),
                source_id: "b".into(),
                target_id: "outside".into(),
            },
        ];
        let mut controller = CanvasController::default();

        controller.copy_selection(&notes, &links);
        let (pasted_notes, pasted_links) = controller.paste_clipboard(24.0, 24.0);

        assert_eq!(pasted_notes.len(), 3);
        assert_eq!(pasted_links.len(), 1);
        let pasted_group = pasted_notes
            .iter()
            .find(|note| note.kind == "group")
            .expect("pasted group");
        assert!(pasted_notes
            .iter()
            .filter(|note| note.kind != "group")
            .all(|note| note.parent_group_id == pasted_group.id));
        assert!(pasted_notes.iter().all(|note| note.selected));
    }

    #[test]
    fn canvas_delete_promotes_group_children_and_removes_node_links() {
        let mut notes = vec![
            CanvasNoteData {
                selected: true,
                ..note("group", "group", 0.0, 0.0)
            },
            CanvasNoteData {
                parent_group_id: "group".into(),
                ..note("child", "text", 20.0, 20.0)
            },
            CanvasNoteData {
                selected: true,
                ..note("removed", "text", 300.0, 20.0)
            },
        ];
        let mut links = vec![CanvasLinkData {
            id: "link".into(),
            source_id: "child".into(),
            target_id: "removed".into(),
        }];

        let removed = remove_selection(&mut notes, &mut links);

        assert_eq!(removed, BTreeSet::from(["group".into(), "removed".into()]));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].parent_group_id.is_empty());
        assert!(links.is_empty());
    }

    #[test]
    fn canvas_selection_rectangle_replaces_or_adds_to_the_selection() {
        let mut notes = vec![
            CanvasNoteData {
                selected: true,
                ..note("outside", "text", 400.0, 400.0)
            },
            note("inside", "text", 40.0, 40.0),
        ];

        select_in_rect(
            &mut notes,
            CanvasRect::normalized(0.0, 0.0, 180.0, 180.0),
            false,
        );
        assert!(!notes[0].selected);
        assert!(notes[1].selected);

        select_in_rect(
            &mut notes,
            CanvasRect::normalized(350.0, 350.0, 550.0, 550.0),
            true,
        );
        assert!(notes.iter().all(|note| note.selected));
    }

    #[test]
    fn canvas_group_selection_wraps_bounds_with_padding() {
        let mut notes = vec![
            CanvasNoteData {
                selected: true,
                ..note("a", "text", 40.0, 60.0)
            },
            CanvasNoteData {
                selected: true,
                ..note("b", "text", 220.0, 180.0)
            },
        ];

        let group_id = group_selection(&mut notes, true).expect("group");
        let group = notes
            .iter()
            .find(|note| note.id == group_id)
            .expect("group note");

        assert_eq!(group.x, 40.0 - GROUP_PADDING);
        assert_eq!(group.y, 60.0 - GROUP_PADDING);
        assert_eq!(group.width, 280.0 + GROUP_PADDING * 2.0);
        assert_eq!(group.height, 200.0 + GROUP_PADDING * 2.0);
    }

    #[test]
    fn canvas_group_resize_does_not_scale_children_and_respects_their_bounds() {
        let mut notes = vec![
            CanvasNoteData {
                width: 500.0,
                height: 400.0,
                ..note("group", "group", 0.0, 0.0)
            },
            CanvasNoteData {
                parent_group_id: "group".into(),
                ..note("child", "text", 300.0, 220.0)
            },
        ];
        let child_before = notes[1].clone();

        assert!(resize_group(&mut notes, "group", 120.0, 90.0));

        assert_eq!(notes[0].width, 300.0 + 100.0 + GROUP_PADDING);
        assert_eq!(notes[0].height, 220.0 + 80.0 + GROUP_PADDING);
        assert_eq!(notes[1], child_before);
    }
}
