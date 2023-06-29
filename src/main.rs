use lyst::{
    mohawk::{Resource, ResourceID, TypeID},
    Mohawk, MohawkReader,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::ExitCode,
    result,
};
use tokio::{
    io::{stdout, AsyncWriteExt},
};

use clap::{Parser, Subcommand};

fn is_4_chars(arg: &str) -> result::Result<TypeID, String> {
    arg.bytes()
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| format!("not 4 ASCII char"))
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct CLI {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List content of given Mohawk file
    List { path: PathBuf },
    /// List content of given Mohawk file
    Extract {
        path: PathBuf,
        #[arg(value_parser = is_4_chars)]
        type_id: TypeID,
        resource_id: ResourceID,
    },
}

mod errors {
    use std::string;

    use tokio::io;

    #[derive(thiserror::Error, Debug)]
    pub enum Error {
        #[error("list: {0}")]
        List(#[from] ListError),
        #[error("extract: {0}")]
        Extract(#[from] ExtractError),
    }

    #[derive(thiserror::Error, Debug)]
    pub enum ListError {
        #[error(transparent)]
        Lyst(#[from] lyst::Error),

        #[error("type not found")]
        InvalidUTF8Name(#[from] string::FromUtf8Error),
    }

    #[derive(thiserror::Error, Debug)]
    pub enum ExtractError {
        #[error(transparent)]
        Lyst(#[from] lyst::Error),

        #[error("type not found")]
        TypeNotFound,
        #[error("resource not found")]
        ResourceNotFound,
        #[error("unsupported type")]
        UnsupportedType,
        #[error("write extracted")]
        WriteExtracted(io::Error),
    }
}

async fn list(path: &Path) -> Result<(), errors::ListError> {
    use errors::ListError::*;

    let print_type = |type_id: &TypeID,
                      resources: &HashMap<ResourceID, Resource>|
     -> Result<_, errors::ListError> {
        println!(
            "{}",
            String::from_utf8(type_id.to_vec()).map_err(InvalidUTF8Name)?,
        );

        let mut sorted_resources: Vec<_> = resources.iter().collect();
        sorted_resources.sort_by_key(|(id, _)| *id);
        println!("   id      name     size flag unknown");
        for (resource_id, resource) in sorted_resources {
            if let Some(name) = &resource.name {
                if name.len() > 9 {
                    panic!("haa");
                }
            }

            println!(
                "{:5} {:<9} {:8}   {:02X}    {:04X}",
                resource_id,
                resource.name.as_ref().unwrap_or(&String::new()),
                resource.file.size,
                resource.file.flag,
                resource.file.unknown,
            );
        }

        Ok(())
    };

    let mut reader = MohawkReader::open(&path).await?;
    let mohawk = Mohawk::with_reader(&mut reader).await?;

    let mut sorted_other_types: Vec<_> = mohawk.types.iter().collect();
    sorted_other_types.sort_by_key(|(t, _)| *t);
    for (type_id, resources) in sorted_other_types {
        print_type(type_id, resources)?;
    }

    Ok(())
}

async fn extract(
    path: impl AsRef<Path>,
    type_id: &TypeID,
    resource_id: &ResourceID,
) -> Result<(), errors::ExtractError> {
    use errors::ExtractError::*;

    let mut reader = MohawkReader::open(&path).await?;
    let mohawk = Mohawk::with_reader(&mut reader).await?;

    let resource = mohawk
        .types
        .get(type_id)
        .ok_or(TypeNotFound)?
        .get(resource_id)
        .ok_or(ResourceNotFound)?;

    match type_id {
        b"MSND" => {
            let raw = resource.with_reader(&mut reader).await?;
            stdout().write_all(&raw).await.map_err(WriteExtracted)
        }
        _ => Err(UnsupportedType),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    /* TODO debug
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
    */
    console_subscriber::init();

    let cli = CLI::parse();

    let ret: Result<(), errors::Error> = match &cli.command {
        Commands::List { path } => list(path).await.map_err(errors::Error::List),
        Commands::Extract {
            path,
            type_id,
            resource_id,
        } => extract(path, type_id, resource_id)
            .await
            .map_err(errors::Error::Extract),
    };

    if let Err(e) = ret {
        eprintln!("{}", e);
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
