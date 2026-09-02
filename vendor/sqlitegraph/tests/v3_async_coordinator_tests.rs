use sqlitegraph::backend::native::v3::AsyncFileCoordinator;
use tempfile::TempDir;

#[tokio::test]
async fn test_async_coordinator_write_and_read() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("async_io.graph");

    let coordinator = AsyncFileCoordinator::create(&db_path).unwrap();

    // Verify initial file size is 0
    assert_eq!(coordinator.file_size(), 0);

    // Write page 1 asynchronously
    let data1 = vec![42u8; 4096];
    let data1_returned = coordinator.write_page(1, data1.clone()).await.unwrap();
    assert_eq!(data1_returned, data1);

    // Write page 2 asynchronously
    let data2 = vec![137u8; 4096];
    let data2_returned = coordinator.write_page(2, data2.clone()).await.unwrap();
    assert_eq!(data2_returned, data2);

    // Read pages back asynchronously in parallel
    let read_buf1 = vec![0u8; 4096];
    let read_buf2 = vec![0u8; 4096];

    let read_future1 = coordinator.read_page(1, read_buf1);
    let read_future2 = coordinator.read_page(2, read_buf2);

    let (res1, res2) = tokio::join!(read_future1, read_future2);

    let (buf1, n1) = res1.unwrap();
    let (buf2, n2) = res2.unwrap();

    assert_eq!(n1, 4096);
    assert_eq!(buf1, data1);

    assert_eq!(n2, 4096);
    assert_eq!(buf2, data2);
}
