//! Adapter al processo esterno ExifTool.
//!
//! ExifTool viene invocato con argomenti strutturati (mai tramite shell) e
//! con il flag `-n` (valori numerici grezzi) sia in lettura sia in scrittura,
//! così i valori non dipendono dalle tabelle di conversione locali.
//!
//! Politica sugli errori: ExifTool usa 0 per il successo, 1 per problemi
//! minori (o file saltati) e 2 per errori seri; poiché anche un errore reale
//! può produrre codice 1, una riga `Error:` in stderr è il segnale
//! definitivo di fallimento.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value};

use crate::error::MetadataError;

/// Mappa `gruppo:nome` → valore, così come prodotta da `-json -G1`.
pub type TagMap = Map<String, Value>;

/// True se l'esecuzione va considerata fallita.
fn esecuzione_fallita(stderr: &str, code: Option<i32>) -> bool {
    if code.unwrap_or(2) >= 2 {
        return true;
    }
    stderr
        .lines()
        .any(|line| line.trim_start().starts_with("Error:"))
}

/// Percorso del sidecar XMP associato a un'immagine (`foto.dng` → `foto.dng.xmp`),
/// coerente con la convenzione usata dalle operazioni filesystem.
pub fn sidecar_path(image: &Path) -> PathBuf {
    let mut name = image
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".xmp");
    image.with_file_name(name)
}

pub struct ExifTool {
    program: PathBuf,
}

impl ExifTool {
    /// Variabile d'ambiente per indicare un percorso esplicito di ExifTool.
    pub const ENV_VAR: &str = "UBERPHOTO_EXIFTOOL";

    /// Individua ExifTool: variabile `UBERPHOTO_EXIFTOOL`, copia inclusa nel
    /// pacchetto accanto all'eseguibile, oppure `exiftool` nel PATH.
    /// Ritorna `None` se nessun candidato è eseguibile.
    pub fn detect() -> Option<Self> {
        if let Some(explicit) = std::env::var_os(Self::ENV_VAR) {
            let tool = Self {
                program: PathBuf::from(explicit),
            };
            return tool.version().ok().map(|version| {
                tracing::debug!(program = %tool.program.display(), %version, "ExifTool individuato");
                tool
            });
        }
        for program in Self::candidati() {
            let tool = Self { program };
            if let Ok(version) = tool.version() {
                tracing::debug!(program = %tool.program.display(), %version, "ExifTool individuato");
                return Some(tool);
            }
        }
        None
    }

    /// Percorsi cercati, in ordine di priorità: prima la copia inclusa nel
    /// pacchetto (accanto all'eseguibile o nelle risorse), poi il PATH.
    fn candidati() -> Vec<PathBuf> {
        let mut percorsi = Vec::new();
        if let Ok(eseguibile) = std::env::current_exe() {
            if let Some(dir) = eseguibile.parent() {
                let nome = if cfg!(windows) {
                    "exiftool.exe"
                } else {
                    "exiftool"
                };
                percorsi.push(dir.join(nome));
                percorsi.push(dir.join("exiftool").join(nome));
                #[cfg(target_os = "macos")]
                percorsi.push(dir.join("../Resources/exiftool/exiftool"));
                #[cfg(target_os = "linux")]
                {
                    percorsi.push(dir.join("../lib/uberphoto/exiftool/exiftool"));
                    percorsi.push(dir.join("../share/uberphoto/exiftool/exiftool"));
                }
            }
        }
        percorsi.push(PathBuf::from(if cfg!(windows) {
            "exiftool.exe"
        } else {
            "exiftool"
        }));
        percorsi
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Versione di ExifTool (es. `"13.50"`).
    pub fn version(&self) -> Result<String, MetadataError> {
        let output = Command::new(&self.program)
            .arg("-ver")
            .output()
            .map_err(MetadataError::Spawn)?;
        if !output.status.success() {
            return Err(MetadataError::Failed {
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// Legge i tag richiesti (elenco vuoto = tutti) per un gruppo di file.
    ///
    /// L'ordine dei risultati segue quello dei percorsi; i file illeggibili
    /// producono una voce con la chiave `Error`, filtrata dai chiamanti.
    pub fn read(&self, paths: &[PathBuf], tags: &[&str]) -> Result<Vec<TagMap>, MetadataError> {
        let mut args: Vec<OsString> = vec![
            "-json".into(),
            "-G1".into(),
            "-struct".into(),
            "-n".into(),
            "-charset".into(),
            "filename=UTF8".into(),
        ];
        for tag in tags {
            args.push(format!("-{tag}").into());
        }
        args.push("--".into());
        for path in paths {
            args.push(path.as_os_str().to_owned());
        }

        let output = Command::new(&self.program)
            .args(&args)
            .output()
            .map_err(MetadataError::Spawn)?;
        match serde_json::from_slice::<Vec<TagMap>>(&output.stdout) {
            Ok(maps) => Ok(maps),
            Err(err) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                if esecuzione_fallita(&stderr, output.status.code()) {
                    Err(MetadataError::Failed {
                        code: output.status.code(),
                        stderr,
                    })
                } else {
                    Err(MetadataError::Json(err))
                }
            }
        }
    }

    /// Scrive i tag nel file, sostituendo l'originale senza backup.
    pub fn write_embedded(&self, path: &Path, assignments: &[String]) -> Result<(), MetadataError> {
        let mut args = Self::write_base();
        for assignment in assignments {
            args.push(format!("-{assignment}").into());
        }
        args.push("--".into());
        args.push(path.as_os_str().to_owned());
        self.run(&args).map(|_| ())
    }

    /// Legge un tag binario (es. `icc_profile`) e ne restituisce il contenuto.
    ///
    /// Usa `-b`: nessuna codifica, nessun JSON; `None` se il tag è assente.
    pub fn read_blob(&self, path: &Path, tag: &str) -> Result<Option<Vec<u8>>, MetadataError> {
        let args: Vec<OsString> = vec![
            "-b".into(),
            format!("-{tag}").into(),
            "--".into(),
            path.as_os_str().to_owned(),
        ];
        let output = self.run(&args)?;
        Ok((!output.is_empty()).then_some(output))
    }

    /// Scrive tag nel file usando argomenti ExifTool completi, dopo le opzioni
    /// di base. Il chiamante include `-tagsfromfile`, selettori, esclusioni e
    /// assegnazioni; il percorso di destinazione è aggiunto in coda.
    ///
    /// Ritorna le righe `Warning:` emesse da ExifTool (senza il prefisso),
    /// utili per il report di esportazione.
    pub fn write_from(
        &self,
        target: &Path,
        args: &[OsString],
    ) -> Result<Vec<String>, MetadataError> {
        let mut comando = Self::write_base();
        comando.extend_from_slice(args);
        comando.push("--".into());
        comando.push(target.as_os_str().to_owned());
        let (_, avvisi) = self.run_dettagliato(&comando)?;
        Ok(avvisi)
    }

    /// Crea il sidecar XMP di un'immagine (RAW), copiando gli XMP già
    /// presenti nel file e applicando le modifiche richieste.
    pub fn create_sidecar(
        &self,
        image: &Path,
        sidecar: &Path,
        assignments: &[String],
    ) -> Result<(), MetadataError> {
        let mut args = Self::write_base();
        args.push("-o".into());
        args.push(sidecar.as_os_str().to_owned());
        args.push("-tagsfromfile".into());
        args.push(image.as_os_str().to_owned());
        args.push("-XMP:all".into());
        for assignment in assignments {
            args.push(format!("-{assignment}").into());
        }
        args.push("--".into());
        args.push(image.as_os_str().to_owned());
        self.run(&args).map(|_| ())
    }

    /// Aggiorna un sidecar esistente, preservandone gli altri campi.
    pub fn update_sidecar(
        &self,
        sidecar: &Path,
        assignments: &[String],
    ) -> Result<(), MetadataError> {
        self.write_embedded(sidecar, assignments)
    }

    fn write_base() -> Vec<OsString> {
        vec![
            "-overwrite_original".into(),
            "-P".into(),
            "-n".into(),
            "-charset".into(),
            "filename=UTF8".into(),
            "-charset".into(),
            "exif=UTF8".into(),
            "-charset".into(),
            "iptc=UTF8".into(),
        ]
    }

    fn run(&self, args: &[OsString]) -> Result<Vec<u8>, MetadataError> {
        self.run_dettagliato(args).map(|(stdout, _)| stdout)
    }

    /// Esegue ExifTool e restituisce stdout e righe di avviso.
    fn run_dettagliato(&self, args: &[OsString]) -> Result<(Vec<u8>, Vec<String>), MetadataError> {
        let output = Command::new(&self.program)
            .args(args)
            .output()
            .map_err(MetadataError::Spawn)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if esecuzione_fallita(&stderr, output.status.code()) {
            return Err(MetadataError::Failed {
                code: output.status.code(),
                stderr: stderr.trim().to_owned(),
            });
        }
        let avvisi = stderr
            .lines()
            .filter_map(|line| line.trim().strip_prefix("Warning:"))
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        Ok((output.stdout, avvisi))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_riga_error_e_il_segnale_definitivo() {
        assert!(!esecuzione_fallita("Warning: [minor] qualcosa", Some(1)));
        assert!(esecuzione_fallita(
            "Error: 'x.xmp' already exists - foto.dng",
            Some(1)
        ));
        assert!(esecuzione_fallita("", Some(2)));
        assert!(!esecuzione_fallita("", Some(0)));
    }

    #[test]
    fn i_candidati_includono_la_copia_inclusa_e_il_path() {
        let candidati = ExifTool::candidati();
        assert!(!candidati.is_empty());
        assert_eq!(
            candidati.last().map(PathBuf::as_path),
            Some(Path::new(if cfg!(windows) {
                "exiftool.exe"
            } else {
                "exiftool"
            }))
        );
        #[cfg(not(windows))]
        assert!(candidati.iter().any(|p| p.ends_with("exiftool/exiftool")));
    }

    #[test]
    fn il_sidecar_append_xmp_al_nome_completo() {
        assert_eq!(
            sidecar_path(Path::new("/foto/IMG_1234.dng")),
            PathBuf::from("/foto/IMG_1234.dng.xmp")
        );
    }
}
