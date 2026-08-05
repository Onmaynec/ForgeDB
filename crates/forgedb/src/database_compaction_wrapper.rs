impl StreamingDatabase {
    /// Выполняет одну synchronous leveled compaction, пересекающую user-key range.
    pub fn compact_range(&self, start: Option<&[u8]>, end: Option<&[u8]>) -> Result<()> {
        self.inner.compact_range(start, end)
    }
}
