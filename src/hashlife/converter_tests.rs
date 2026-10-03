    use super::{frontend_to_hashlife, grid_to_hashlife, hashlife_to_frontend, hashlife_to_grid,
               FRONTEND_OFFSET, GRID_OFFSET, HASHLIFE_OFFSET};

    /// Each space's origin (its offset constant) maps to the hashlife origin,
    /// and the inverse maps back.
    #[test]
    fn test_converter_origin_mapping() {
        assert_eq!(grid_to_hashlife(GRID_OFFSET, GRID_OFFSET), (HASHLIFE_OFFSET, HASHLIFE_OFFSET));
        assert_eq!(frontend_to_hashlife(FRONTEND_OFFSET, FRONTEND_OFFSET), (HASHLIFE_OFFSET, HASHLIFE_OFFSET));
        assert_eq!(hashlife_to_grid(HASHLIFE_OFFSET, HASHLIFE_OFFSET), (GRID_OFFSET, GRID_OFFSET));
        assert_eq!(hashlife_to_frontend(HASHLIFE_OFFSET, HASHLIFE_OFFSET), (FRONTEND_OFFSET, FRONTEND_OFFSET));
    }

    /// Converting a->b->a is the identity at representative points: each
    /// offset's origin, ±1, and large offsets.
    #[test]
    fn test_converter_roundtrip() {
        let pts: Vec<(u64, u64)> = vec![
            (GRID_OFFSET, GRID_OFFSET),
            (GRID_OFFSET + 1, GRID_OFFSET - 1),
            (GRID_OFFSET + 123_456, GRID_OFFSET + 78_901),
            (FRONTEND_OFFSET, FRONTEND_OFFSET),
            (FRONTEND_OFFSET + 511, FRONTEND_OFFSET - 510),
            (HASHLIFE_OFFSET, HASHLIFE_OFFSET),
            (HASHLIFE_OFFSET + 1, HASHLIFE_OFFSET - 1),
            (HASHLIFE_OFFSET + 1_000_000_000, HASHLIFE_OFFSET + 999_999_999),
        ];
        for &(x, y) in &pts {
            let (hx, hy) = frontend_to_hashlife(x, y);
            assert_eq!(hashlife_to_frontend(hx, hy), (x, y),
                "frontend->hashlife->frontend roundtrip at ({}, {})", x, y);
            let (gx, gy) = grid_to_hashlife(x, y);
            assert_eq!(hashlife_to_grid(gx, gy), (x, y),
                "grid->hashlife->grid roundtrip at ({}, {})", x, y);
        }
    }

    /// A logical offset d is the same in every space: converting
    /// (space_origin + d) always yields (hashlife_origin + d).
    #[test]
    fn test_converter_translation_invariance() {
        for d in [1i64, 100, 1_000_000, -1, -100, -1_000_000] {
            let o = HASHLIFE_OFFSET as i64;
            let g = grid_to_hashlife((GRID_OFFSET as i64 + d) as u64, GRID_OFFSET);
            assert_eq!((g.0 as i64 - o, g.1 as i64 - o), (d, 0));
            let f = frontend_to_hashlife((FRONTEND_OFFSET as i64 + d) as u64, FRONTEND_OFFSET);
            assert_eq!((f.0 as i64 - o, f.1 as i64 - o), (d, 0));
        }
    }
