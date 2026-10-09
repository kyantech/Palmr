macro_rules! folder_hidden_sql {
    ($folder:expr, $owner:expr) => {
        concat!(
            "(EXISTS (SELECT 1 FROM folders z WHERE z.owner_id = ",
            $owner,
            " AND z.deleting = 1) AND EXISTS (WITH RECURSIVE up(id, parent_id, deleting, level) AS (",
            "SELECT a.id, a.parent_id, a.deleting, 0 FROM folders a WHERE a.id = ",
            $folder,
            " AND a.owner_id = ",
            $owner,
            " UNION ALL SELECT p.id, p.parent_id, p.deleting, up.level + 1 FROM folders p ",
            "JOIN up ON p.id = up.parent_id WHERE p.owner_id = ",
            $owner,
            " AND up.level < 64) SELECT 1 FROM up WHERE deleting = 1))"
        )
    };
}

pub(crate) use folder_hidden_sql;

#[cfg(test)]
mod tests {
    #[test]
    fn unit_hidden_fragment_is_bounded_and_owner_scoped_at_every_level() {
        let sql = folder_hidden_sql!("x.folder_id", "x.owner_id");
        assert!(sql.contains("up.level < 64"));
        assert_eq!(sql.matches("x.owner_id").count(), 3);
        assert!(sql.contains("z.deleting = 1"));
    }
}
