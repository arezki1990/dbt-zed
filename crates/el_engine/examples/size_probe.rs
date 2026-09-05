//! Binary-size probe: minimal executable pulling the engine's polars surface.

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: size_probe <csv>");
    let root = std::path::Path::new(".");
    let mut extractor = el_engine::connectors::files::FileExtractor::new(
        root,
        &path,
        el_engine::spec::FileFormat::Csv,
        None,
        10_000,
    )?;
    use el_engine::connectors::Extractor as _;
    let mut rows = 0usize;
    while let Some(chunk) = extractor.next_chunk()? {
        rows += chunk.height();
    }
    println!("{rows} rows");
    Ok(())
}
