//! Binary-size probe for the spike: a minimal executable that pulls the
//! full el_engine polars surface, so `ls -lh` on the release artifact
//! approximates what linking the engine adds to zdbt.

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: size_probe <csv>");
    let df = el_engine::read_csv(std::path::Path::new(&path))?;
    let outcome = el_engine::apply_casts(
        df,
        &[el_engine::CastRule {
            column: "id".into(),
            to: polars::prelude::DataType::Int64,
            strict: false,
            parse: None,
        }],
    )?;
    println!("{} rows, {} lax failures", outcome.df.height(), outcome.lax_failures.len());
    Ok(())
}
