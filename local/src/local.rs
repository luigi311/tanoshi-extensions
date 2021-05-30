use anyhow::{anyhow, Result};
use fancy_regex::Regex;
use std::io::BufReader;
use std::{fs, io};
use tanoshi_lib::extensions::Extension;
use tanoshi_lib::model::{Chapter, Manga, SortByParam, SortOrderParam, Source};

pub static ID: i64 = 1;
pub static NAME: &str = "local";

#[derive(Default)]
pub struct Local {
    pub url: String,
}

impl Local {
    pub fn new(path: &String) -> Local {
        Local {
            url: path.to_string(),
        }
    }
}

impl Extension for Local {
    fn detail(&self) -> Source {
        Source {
            id: ID,
            name: NAME.to_string(),
            url: self.url.clone(),
            version: std::env!("PLUGIN_VERSION").to_string(),
            icon: "".to_string(),
            need_login: false,
        }
    }

    fn get_mangas(
        &self,
        keyword: Option<String>,
        genres: Option<Vec<String>>,
        page: Option<i32>,
        sort_by: Option<SortByParam>,
        sort_order: Option<SortOrderParam>,
        auth: Option<String>,
    ) -> Result<Vec<Manga>> {
        let local_path = self.url.clone();
        let entries = fs::read_dir(&self.url)
            .expect("error read directory")
            .filter(|res| res.as_ref().unwrap().file_type().unwrap().is_dir())
            .map(|res| {
                res.map(|e| Manga {
                    source_id: ID,
                    title: e.file_name().to_str().unwrap().to_string(),
                    author: vec![],
                    genre: vec![],
                    status: None,
                    description: None,
                    path: e
                        .path()
                        .to_str()
                        .unwrap()
                        .replace(local_path.as_str(), "")
                        .to_string(),
                    cover_url: "".to_string(),
                })
            })
            .collect::<Result<Vec<_>, io::Error>>()
            .unwrap_or(vec![]);

        Ok(entries)
    }

    fn get_manga_info(&self, _path: &String) -> Result<Manga> {
        Ok(Manga::default())
    }

    fn get_chapters(&self, path: &String) -> Result<Vec<Chapter>> {
        let vol_re = Regex::new(r"(?i)(?<=v)(\d+)|(?<=volume)\s*(\d+)|(?<=vol)\s*(\d+)").unwrap();
        let ch_re = Regex::new(r"(?i)(?<=ch)(\d+)|(?<=chapter)\s*(\d+)").unwrap();

        let url = format!("{}{}", &self.url, &path);
        let local_path = self.url.clone();
        let entries = fs::read_dir(url)?
            .filter(|res| {
                res.as_ref().unwrap().file_type().unwrap().is_file()
                    && !res
                        .as_ref()
                        .unwrap()
                        .file_name()
                        .as_os_str()
                        .to_str()
                        .unwrap()
                        .starts_with(".")
            })
            .map(|res| {
                res.map(|e| {
                    // let mut ch = Chapter {
                    //     source_id: ID,
                    //     manga_id: 0,
                    //     title: "".to_string(),
                    //     path: "".to_string(),
                    //     rank: 0,
                    //     uploaded: (),
                    // };
                    let file_name = e.file_name().to_str().unwrap().to_string();
                    let mat = vol_re.find(file_name.as_str()).unwrap();
                    // ch.vol = mat.map(|m| m.as_str().to_string());
                    let mat = ch_re.find(file_name.as_str()).unwrap();
                    // ch.no = mat.map(|m| m.as_str().to_string());
                    // ch.title = Some(file_name);
                    // ch.source = NAME.to_string();
                    // ch.path = e
                    //     .path()
                    //     .to_str()
                    //     .unwrap()
                    //     .replace(local_path.as_str(), "")
                    //     .to_string();
                    let created = e
                        .metadata()
                        .unwrap()
                        .created()
                        .unwrap()
                        .duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    Chapter {
                        source_id: ID,
                        title: file_name,
                        path: e
                            .path()
                            .to_str()
                            .unwrap()
                            .replace(local_path.as_str(), "")
                            .to_string(),
                        number: 0.0,
                        scanlator: "".to_string(),
                        uploaded: chrono::NaiveDateTime::from_timestamp(created as i64, 0),
                    }
                })
            })
            .collect::<Result<Vec<_>, io::Error>>()
            .unwrap_or(vec![]);

        Ok(entries)
    }

    fn get_pages(&self, path: &String) -> Result<Vec<String>> {
        let url = format!("{}{}", &self.url, &path);
        let file = fs::File::open(&url).unwrap();
        let reader = BufReader::new(file);

        let archive = zip::ZipArchive::new(reader).unwrap();
        Ok(archive
            .file_names()
            .map(|file_name| format!("{}/{}", url, file_name))
            .collect())
    }

    fn get_page(&self, url: &String) -> Result<Vec<u8>> {
        let path = std::path::Path::new(&url);
        let dir = path.parent().unwrap().to_str().unwrap();
        let file_name = path.file_name().unwrap().to_str().unwrap();
        println!("{}", &dir);
        println!("{}", &file_name);

        let file = match fs::File::open(&dir) {
            Ok(file) => file,
            Err(e) => {
                return Err(anyhow!("error open file: {}", e));
            }
        };

        let reader = BufReader::new(file);

        let mut archive = zip::ZipArchive::new(reader)?;
        let mut zip_file = archive.by_name(file_name)?;
        let mut bytes = vec![];
        if io::copy(&mut zip_file, &mut bytes).is_err() {
            return Err(anyhow!("error write image"));
        }

        Ok(bytes)
    }

    fn login(
        &self,
        _: tanoshi_lib::model::SourceLogin,
    ) -> Result<tanoshi_lib::model::SourceLoginResult> {
        Err(anyhow!("not implemented"))
    }
}
