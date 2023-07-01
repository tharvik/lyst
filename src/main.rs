use lyst::{
    mohawk::{Resource, ResourceID, TypeID},
    Mohawk,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::ExitCode,
    result,
};
use tokio::io::{self, stdout};

use clap::{Parser, Subcommand};

fn is_4_chars(arg: &str) -> result::Result<TypeID, String> {
    let raw: [u8; 4] = arg
        .bytes()
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| format!("not 4 ASCII char"))?;

    Ok(TypeID::from(raw))
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
    let print_type = |type_id: &TypeID,
                      resources: &HashMap<ResourceID, Resource>|
     -> Result<_, errors::ListError> {
        println!("{}", type_id);

        let mut sorted_resources: Vec<_> = resources.iter().collect();
        sorted_resources.sort_unstable_by_key(|(id, _)| *id);
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

    let mohawk = Mohawk::open(&path).await?;

    let mut sorted_other_types: Vec<_> = mohawk.types.iter().collect();
    sorted_other_types.sort_unstable_by_key(|(t, _)| *t);
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

    let mohawk = Mohawk::open(path).await?;

    match type_id {
        TypeID::MSND => {
            let resource = mohawk
                .types
                .get(type_id)
                .ok_or(TypeNotFound)?
                .get(resource_id)
                .ok_or(ResourceNotFound)?;

            io::copy(&mut resource.read(), &mut stdout())
                .await
                .map(|_| ())
                .map_err(WriteExtracted)?;
        }
        TypeID::PICT => {
            mohawk
                .get_pict(resource_id)
                .await
                .ok_or(ResourceNotFound)??;
        }
        _ => return Err(UnsupportedType),
    }

    Ok(())
}

#[cfg(not(feature = "debug"))]
fn setup_tracing() {
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::TRACE)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");
}

#[cfg(feature = "debug")]
fn setup_tracing() {
    console_subscriber::init();
}

#[tokio::main]
async fn main() -> ExitCode {
    setup_tracing();

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
