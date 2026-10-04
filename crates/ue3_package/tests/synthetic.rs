use ue3_package::reader::Reader;

#[test]
fn fstring_ansi_and_utf16() {
    let mut b = vec![5, 0, 0, 0];
    b.extend_from_slice(b"None\0");
    b.extend_from_slice(&(-3i32).to_le_bytes());
    for c in "hi\0".encode_utf16() {
        b.extend_from_slice(&c.to_le_bytes());
    }
    let mut r = Reader::new(&b);
    assert_eq!(r.fstring().unwrap(), "None");
    assert_eq!(r.fstring().unwrap(), "hi");
    assert_eq!(r.remaining(), 0);
}

#[test]
fn bulk_array_bounds() {
    let mut b = Vec::new();
    b.extend_from_slice(&2i32.to_le_bytes());
    b.extend_from_slice(&3i32.to_le_bytes());
    b.extend_from_slice(&[1, 0, 2, 0, 3, 0]);
    let (esz, n, data) = Reader::new(&b).bulk_array().unwrap();
    assert_eq!((esz, n, data.len()), (2, 3, 6));
    // Count larger than the buffer must fail, not allocate.
    let mut bad = Vec::new();
    bad.extend_from_slice(&4i32.to_le_bytes());
    bad.extend_from_slice(&1_000_000i32.to_le_bytes());
    assert!(Reader::new(&bad).bulk_array().is_err());
}

#[test]
fn rejects_non_package() {
    assert!(ue3_package::Package::from_bytes("x".into(), vec![0u8; 64]).is_err());
}
