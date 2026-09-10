use super::*;
use crate::css::{Origin, parse_stylesheet};

#[test]
fn column_offsets_preserve_float_addition_order() {
    assert_eq!(column_x_offsets(&[], 2.0), Vec::<f32>::new());
    assert_eq!(
        column_x_offsets(&[16_777_216.0, 1.0, 1.0, 6.0, 0.5, 0.25], 0.5),
        vec![
            0.0,
            16_777_216.0,
            16_777_218.0,
            16_777_220.0,
            16_777_226.0,
            16_777_228.0
        ]
    );
}

#[test]
fn wide_tables_compute_each_column_offset_once_for_all_rows() {
    for (rows, columns) in [(2, 32), (8, 32), (8, 128)] {
        let body = NodeHandle::element("body");
        let table = NodeHandle::element("table");
        body.append_child(table.clone());
        for _ in 0..rows {
            let row = NodeHandle::element("tr");
            for column in 0..columns {
                let cell = NodeHandle::element("td");
                cell.set_attribute(
                    "style",
                    format!("width: {}px", 10.0 + (column % 5) as f32 * 0.25),
                );
                row.append_child(cell);
            }
            table.append_child(row);
        }
        let content_width: f32 = (0..columns).map(|c| 10.0 + (c % 5) as f32 * 0.25).sum();
        let width = content_width + (columns + 1) as f32 * 1.5;
        let mut resolver = StyleResolver::new();
        resolver.add_stylesheet(Origin::Author, parse_stylesheet(&format!(
            "body {{ display: block; margin: 0; }} table {{ display: table; width: {width}px; border-spacing: 1.5px; }} tr {{ display: table-row; }} td {{ display: table-cell; height: 2px; }}"
        )).unwrap());
        COLUMN_OFFSET_ADDITIONS.with(|count| count.set(0));
        let layout = crate::layout::layout_tree(
            &body,
            &mut resolver,
            Rect {
                x: 0.0,
                y: 0.0,
                width,
                height: 0.0,
            },
        )
        .unwrap();
        let table = &layout.children[0];
        assert_eq!(table.children.len(), rows);
        for row in &table.children {
            assert_eq!(row.children.len(), columns);
            let mut expected = table.dimensions.content.x + 1.5;
            for cell in &row.children {
                assert_eq!(cell.dimensions.content.x, expected);
                expected += cell.dimensions.content.width + 1.5;
            }
        }
        let additions = COLUMN_OFFSET_ADDITIONS.with(std::cell::Cell::get);
        eprintln!("table rows={rows} columns={columns} offset_additions={additions}");
        assert_eq!(additions, columns);
    }
}
