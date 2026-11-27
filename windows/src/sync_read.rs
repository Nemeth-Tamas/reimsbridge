pub enum ReadOutcome<T = String> {
    Data(T),
    Unavailable,
    Changed,
}
