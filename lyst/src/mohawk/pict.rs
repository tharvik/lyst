#[cfg(test)]
mod tests {
    use std::path::Path;

    use tokio_stream::StreamExt;

    use crate::{mohawk::TypeID, tests::get_known_files, Mohawk};

    #[test_log::test(tokio::test)]
    #[ignore]
    async fn parse_all_known() {
        async fn run(path: impl AsRef<Path>) {
            let mohawk = Mohawk::open(path).await.expect("open mohawk");

            for id in mohawk
                .types
                .get(&TypeID::PICT)
                .expect("contain PICT")
                .keys()
            {
                mohawk.get_pict(id).await.unwrap().expect("parse PICT");
            }
        }

        get_known_files()
            .filter(|p| p.file_name().unwrap().to_str().unwrap() != "CREDITS.DAT")
            .then(run)
            .collect::<()>()
            .await;
    }
}
