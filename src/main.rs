use lyst::{
    mohawk::{pict::PICT, Resource, ResourceID, TypeID},
    Mohawk,
};
use sdl2::image::LoadTexture;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::ExitCode,
    result,
    time::Duration,
};

use tokio::{
    io::{self, stdout},
    task::spawn_blocking,
};

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
    use lyst::mohawk;
    use tokio::{io, task};

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
        Mohawk(#[from] mohawk::Error),
    }

    #[derive(thiserror::Error, Debug)]
    pub enum ExtractError {
        #[error(transparent)]
        Mohawk(#[from] mohawk::Error),

        #[error("type not found")]
        TypeNotFound,
        #[error("resource not found")]
        ResourceNotFound,
        #[error("unsupported type")]
        UnsupportedType,
        #[error("write extracted: {0}")]
        WriteExtracted(io::Error),
        #[error("setup pict show: {0}")]
        SetupPictShow(task::JoinError),
        #[error("show extracted picture: {0}")]
        ShowPict(String),
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

fn show_pict(pict: PICT) -> Result<(), String> {
    use sdl2::{event::Event, keyboard::Keycode};

    let sdl_context = sdl2::init()?;
    let window = sdl_context
        .video()?
        .window("rust-sdl2 demo", 800, 600)
        .build()
        .unwrap();

    let mut canvas = window.into_canvas().build().unwrap();

    let texture_creator = canvas.texture_creator();
    let texture = texture_creator.load_texture_bytes(pict.as_ref()).unwrap();

    canvas.clear();
    let mut event_pump = sdl_context.event_pump().unwrap();
    'running: loop {
        canvas.copy(&texture, None, None).unwrap();
        canvas.present();
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. }
                | Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => break 'running Ok(()),
                _ => {}
            }
        }
        // The rest of the game loop goes here...

        canvas.present();
        ::std::thread::sleep(Duration::new(0, 1_000_000_000u32 / 60));
    }
}

async fn extract(
    path: impl AsRef<Path>,
    type_id: &TypeID,
    resource_id: &ResourceID,
) -> Result<(), errors::ExtractError> {
    use errors::ExtractError::*;

    let mohawk = crate::Mohawk::open(path).await?;

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
            let pict = mohawk
                .get_pict(resource_id)
                .await
                .ok_or(ResourceNotFound)??;

            spawn_blocking(|| show_pict(pict))
                .await
                .map_err(SetupPictShow)?
                .map_err(ShowPict)?;
        }
        _ => return Err(UnsupportedType),
    }

    Ok(())
}

#[cfg(not(feature = "dep:console-subscriber"))]
fn setup_tracing() {
    tracing_subscriber::fmt::init();
}

#[cfg(feature = "dep:console-subscriber")]
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
