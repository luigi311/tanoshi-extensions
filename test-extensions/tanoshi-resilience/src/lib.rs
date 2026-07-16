use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use bytes::Bytes;
use tanoshi_lib::prelude::{
    ChapterInfo, Extension, Input, Lang, MangaInfo, PluginRegistrar, SourceInfo,
};

const SOURCE_ID: i64 = 9_000;
const SOURCE_NAME: &str = "Tanoshi Test";
const SOURCE_URL: &str = "test://tanoshi-resilience";
const SLEEP_DURATION: Duration = Duration::from_millis(100);
const BLOCK_DEADLINE: Duration = Duration::from_secs(30);
const BLOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);

tanoshi_lib::export_plugin!(register);

fn register(registrar: &mut dyn PluginRegistrar) {
    registrar.register_function(Box::new(TanoshiResilience::default()));
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Behavior {
    #[default]
    Normal,
    Sleep,
    Block,
    PanicRead,
    Error,
}

impl Behavior {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "normal" => Ok(Self::Normal),
            "sleep" => Ok(Self::Sleep),
            "block" => Ok(Self::Block),
            "panic_read" => Ok(Self::PanicRead),
            "error" => Ok(Self::Error),
            "panic_preferences" => {
                panic!("tanoshi test extension preference panic")
            }
            "error_preferences" => {
                bail!("tanoshi test extension deterministic preference error")
            }
            _ => bail!("unknown tanoshi test extension mode: {value}"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Sleep => "sleep",
            Self::Block => "block",
            Self::PanicRead => "panic_read",
            Self::Error => "error",
        }
    }
}

#[derive(Default)]
struct TanoshiResilience {
    behavior: Behavior,
    release_file: Option<PathBuf>,
}

impl TanoshiResilience {
    fn run_read_behavior(&self, operation: &str) -> Result<()> {
        match self.behavior {
            Behavior::Normal => Ok(()),
            Behavior::Sleep => {
                thread::sleep(SLEEP_DURATION);
                Ok(())
            }
            Behavior::Block => self.wait_for_release(operation),
            Behavior::PanicRead => panic!("tanoshi test extension read panic: {operation}"),
            Behavior::Error => {
                bail!("tanoshi test extension deterministic read error: {operation}")
            }
        }
    }

    fn wait_for_release(&self, operation: &str) -> Result<()> {
        let release_file = self.release_file.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "tanoshi test extension block mode requires test_release_file: {operation}"
            )
        })?;
        let deadline = Instant::now() + BLOCK_DEADLINE;

        while !release_file.exists() {
            if Instant::now() >= deadline {
                bail!(
                    "tanoshi test extension block deadline expired: {operation} ({})",
                    release_file.display()
                );
            }
            thread::sleep(BLOCK_POLL_INTERVAL);
        }

        Ok(())
    }

    fn deterministic_manga(&self, path: String) -> MangaInfo {
        MangaInfo {
            source_id: SOURCE_ID,
            title: "Tanoshi Test Manga".to_string(),
            author: vec!["Tanoshi Test".to_string()],
            genre: vec!["fixture".to_string()],
            status: Some("complete".to_string()),
            description: Some("A deterministic test manga.".to_string()),
            path,
            cover_url: "test://tanoshi-resilience/cover".to_string(),
        }
    }

    fn deterministic_chapter(&self, path: String) -> ChapterInfo {
        ChapterInfo {
            source_id: SOURCE_ID,
            title: "Chapter 1".to_string(),
            path,
            number: 1.0,
            scanlator: Some("Tanoshi Test".to_string()),
            uploaded: 0,
        }
    }
}

impl Extension for TanoshiResilience {
    fn get_source_info(&self) -> SourceInfo {
        SourceInfo {
            id: SOURCE_ID,
            name: SOURCE_NAME.to_string(),
            url: SOURCE_URL.to_string(),
            version: env!("CARGO_PKG_VERSION"),
            icon: "",
            languages: Lang::All,
            nsfw: false,
        }
    }

    fn filter_list(&self) -> Vec<Input> {
        vec![]
    }

    fn get_preferences(&self) -> Result<Vec<Input>> {
        Ok(vec![
            Input::Text {
                name: "test_mode".to_string(),
                state: Some(self.behavior.name().to_string()),
            },
            Input::Text {
                name: "test_release_file".to_string(),
                state: self
                    .release_file
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned()),
            },
        ])
    }

    fn set_preferences(&mut self, preferences: Vec<Input>) -> Result<()> {
        let mut behavior = self.behavior;
        let mut release_file = self.release_file.clone();

        for preference in preferences {
            match preference {
                Input::Text { name, state } if name == "test_mode" => {
                    let value = state
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("test_mode requires a value"))?;
                    behavior = Behavior::parse(value)?;
                }
                Input::Text { name, state } if name == "test_release_file" => {
                    release_file = state.map(PathBuf::from);
                }
                _ => bail!("unsupported tanoshi test extension preference"),
            }
        }

        self.behavior = behavior;
        self.release_file = release_file;
        Ok(())
    }

    fn get_popular_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        self.run_read_behavior("get_popular_manga")?;
        Ok(vec![self.deterministic_manga(format!("popular/{page}"))])
    }

    fn get_latest_manga(&self, page: i64) -> Result<Vec<MangaInfo>> {
        self.run_read_behavior("get_latest_manga")?;
        Ok(vec![self.deterministic_manga(format!("latest/{page}"))])
    }

    fn search_manga(
        &self,
        _page: i64,
        _query: Option<String>,
        _filters: Option<Vec<Input>>,
    ) -> Result<Vec<MangaInfo>> {
        self.run_read_behavior("search_manga")?;
        Ok(vec![self.deterministic_manga("search".to_string())])
    }

    fn get_manga_detail(&self, path: String) -> Result<MangaInfo> {
        self.run_read_behavior("get_manga_detail")?;
        Ok(self.deterministic_manga(path))
    }

    fn get_chapters(&self, path: String) -> Result<Vec<ChapterInfo>> {
        self.run_read_behavior("get_chapters")?;
        Ok(vec![self.deterministic_chapter(path)])
    }

    fn get_pages(&self, path: String) -> Result<Vec<String>> {
        self.run_read_behavior("get_pages")?;
        Ok(vec![format!("test://tanoshi-resilience/page/{path}")])
    }

    fn get_image_bytes(&self, _url: String) -> Result<Bytes> {
        self.run_read_behavior("get_image_bytes")?;
        Ok(Bytes::from_static(b"tanoshi-test-image"))
    }
}
