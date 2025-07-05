use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "frontend/"]
pub struct Assets;

impl Assets {
    pub fn get_file(path: &str) -> Option<rust_embed::EmbeddedFile> {
        Assets::get(path)
    }

    pub fn get_content_type(path: &str) -> String {
        mime_guess::from_path(path).first_or_octet_stream().to_string()
    }
}
