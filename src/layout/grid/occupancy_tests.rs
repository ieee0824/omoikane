use super::*;

#[test]
fn sparse_single_column_occupancy_updates_grow_linearly() {
    for count in [128, 256, 512] {
        let automatic = AxisRequest {
            start: None,
            span: 1,
        };
        let requests = vec![(automatic, automatic); count];
        let mut columns = vec![Track::auto()];
        let mut rows = Vec::new();
        OCCUPANCY_ROW_UPDATES.with(|work| work.set(0));
        let placements = place_row_major(&requests, &mut columns, &mut rows, false);
        let updates = OCCUPANCY_ROW_UPDATES.with(std::cell::Cell::get);
        for (index, placement) in placements.iter().enumerate() {
            assert_eq!((placement.column, placement.row), (0, index));
        }
        assert!(updates <= count * 2, "{count} items updated {updates} rows");
    }
}

#[test]
fn occupancy_growth_preserves_cells_and_initializes_new_area() {
    let mut occupied = Vec::new();
    ensure_occupancy(&mut occupied, 2, 1);
    occupied[0][0] = true;
    ensure_occupancy(&mut occupied, 3, 4);
    assert_eq!(
        occupied,
        [
            vec![true, false, false, false],
            vec![false; 4],
            vec![false; 4]
        ]
    );
    OCCUPANCY_ROW_UPDATES.with(|work| work.set(0));
    ensure_occupancy(&mut occupied, 3, 4);
    assert_eq!(OCCUPANCY_ROW_UPDATES.with(std::cell::Cell::get), 0);
    ensure_occupancy(&mut occupied, 5, 4);
    assert_eq!(OCCUPANCY_ROW_UPDATES.with(std::cell::Cell::get), 2);
    assert!(occupied[0][0]);
    assert_eq!(&occupied[3..], [vec![false; 4], vec![false; 4]]);
}

#[test]
fn placement_preserves_spans_dense_holes_and_column_flow() {
    for column_flow in [false, true] {
        for dense in [false, true] {
            let mut requests = vec![
                (
                    AxisRequest {
                        start: Some(0),
                        span: 1,
                    },
                    AxisRequest {
                        start: Some(0),
                        span: 2,
                    },
                ),
                (
                    AxisRequest {
                        start: None,
                        span: 2,
                    },
                    AxisRequest {
                        start: None,
                        span: 1,
                    },
                ),
                (
                    AxisRequest {
                        start: None,
                        span: 1,
                    },
                    AxisRequest {
                        start: None,
                        span: 1,
                    },
                ),
            ];
            let (mut columns, mut rows) = (vec![Track::auto(); 2], Vec::new());
            if column_flow {
                requests
                    .iter_mut()
                    .for_each(|request| std::mem::swap(&mut request.0, &mut request.1));
                std::mem::swap(&mut columns, &mut rows);
            }
            let placements = place_items(&requests, &mut columns, &mut rows, column_flow, dense);
            let positions: Vec<_> = placements
                .iter()
                .map(|p| {
                    if column_flow {
                        (p.row, p.column, p.row_span, p.column_span)
                    } else {
                        (p.column, p.row, p.column_span, p.row_span)
                    }
                })
                .collect();
            let last = if dense { (1, 0, 1, 1) } else { (0, 3, 1, 1) };
            assert_eq!(positions, [(0, 0, 1, 2), (0, 2, 2, 1), last]);
        }
    }
}

#[test]
fn definite_column_growth_keeps_earlier_occupied_cells() {
    let requests = [
        (
            AxisRequest {
                start: Some(0),
                span: 1,
            },
            AxisRequest {
                start: Some(0),
                span: 1,
            },
        ),
        (
            AxisRequest {
                start: Some(3),
                span: 1,
            },
            AxisRequest {
                start: Some(2),
                span: 1,
            },
        ),
        (
            AxisRequest {
                start: None,
                span: 2,
            },
            AxisRequest {
                start: None,
                span: 1,
            },
        ),
    ];
    let placements = place_row_major(&requests, &mut vec![Track::auto()], &mut Vec::new(), false);
    let positions: Vec<_> = placements.iter().map(|p| (p.column, p.row)).collect();
    assert_eq!(positions, [(0, 0), (3, 2), (1, 0)]);
}
