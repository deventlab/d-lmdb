use super::ConsistencyLevel;
use super::ConsistencyParam;

fn parse(query: &str) -> Result<ConsistencyParam, serde_urlencoded::de::Error> {
    serde_urlencoded::from_str(query)
}

#[test]
fn test_absent_level_defaults_to_eventual() {
    assert_eq!(parse("").unwrap().level, ConsistencyLevel::Eventual);
}

#[test]
fn test_level_eventual() {
    assert_eq!(
        parse("level=eventual").unwrap().level,
        ConsistencyLevel::Eventual
    );
}

#[test]
fn test_level_linearizable() {
    assert_eq!(
        parse("level=linearizable").unwrap().level,
        ConsistencyLevel::Linearizable
    );
}

#[test]
fn test_level_lease() {
    assert_eq!(parse("level=lease").unwrap().level, ConsistencyLevel::Lease);
}

#[test]
fn test_unknown_level_is_rejected() {
    // This is what makes the "forgot to wire up a new level" bug class impossible:
    // the enum is exhaustively matched in get_handler, and anything that doesn't
    // deserialize into a known variant fails right here, not silently falls through.
    assert!(parse("level=bogus").is_err());
}
