use bytes::Bytes;
use networking::RateLimitedAgent;
use tanoshi_lib::prelude::{Extension, Input, PluginRegistrar};

#[doc(hidden)]
pub use anyhow;
#[doc(hidden)]
pub use bytes;
#[doc(hidden)]
pub use tanoshi_lib;

/// Fetch a page through the extension and fully decode it as an image.
/// Enable `test-support` only through dev-dependencies so the decoder stays
/// out of released extensions. Sample a page instead of downloading a chapter.
#[cfg(feature = "test-support")]
pub fn assert_valid_page_image(extension: &impl Extension, url: &str) {
    let bytes = extension
        .get_image_bytes(url.to_string())
        .unwrap_or_else(|error| panic!("failed to download page image {url}: {error:#}"));
    let image = image::load_from_memory(&bytes)
        .unwrap_or_else(|error| panic!("page {url} did not decode as an image: {error}"));
    assert!(
        image.width() > 0 && image.height() > 0,
        "page {url} has empty image dimensions"
    );
}

/// Merge preference updates into an extension's declared preferences.
///
/// Matching deliberately uses [`Input::eq`]. In `tanoshi-lib` 0.38.0,
/// `PartialEq` matches the input variant and name while ignoring its state and
/// available values. Updates for undeclared preferences are therefore ignored.
pub fn merge_preferences(preferences: &mut [Input], updates: Vec<Input>) {
    for update in updates {
        for preference in preferences.iter_mut() {
            if update.eq(preference) {
                *preference = update.clone();
            }
        }
    }
}

#[doc(hidden)]
pub fn register_extension<E>(
    registrar: &mut dyn PluginRegistrar,
    name: &str,
    version: &str,
    log_target: &str,
) where
    E: Default + Extension + 'static,
{
    networking::init_plugin_logging();
    log::info!(target: log_target, "Registering {name} extension v{version}");
    registrar.register_function(Box::new(E::default()));
}

#[doc(hidden)]
pub fn fetch_direct_image(
    client: &RateLimitedAgent,
    name: &str,
    url: String,
    referer: &str,
    log_target: &str,
) -> anyhow::Result<Bytes> {
    log::debug!(target: log_target, "{name}: get_image_bytes url={url}");
    client.fetch_bytes(&url, Some(referer))
}

/// Generate the exported plugin declaration and its registration function.
#[macro_export]
macro_rules! export_extension {
    ($register:ident, $extension:ty, $name:expr $(,)?) => {
        $crate::tanoshi_lib::export_plugin!($register);

        fn $register(registrar: &mut dyn $crate::tanoshi_lib::prelude::PluginRegistrar) {
            $crate::register_extension::<$extension>(
                registrar,
                $name,
                env!("CARGO_PKG_VERSION"),
                module_path!(),
            );
        }
    };
}

/// Generate `Extension` preference methods backed by a `Vec<Input>` field.
#[macro_export]
macro_rules! impl_preferences {
    ($field:ident $(,)?) => {
        fn set_preferences(
            &mut self,
            preferences: ::std::vec::Vec<$crate::tanoshi_lib::prelude::Input>,
        ) -> $crate::anyhow::Result<()> {
            $crate::merge_preferences(&mut self.$field, preferences);
            ::core::result::Result::Ok(())
        }

        fn get_preferences(
            &self,
        ) -> $crate::anyhow::Result<::std::vec::Vec<$crate::tanoshi_lib::prelude::Input>> {
            ::core::result::Result::Ok(self.$field.clone())
        }
    };
}

/// Generate direct image fetching through a `RateLimitedAgent` field.
#[macro_export]
macro_rules! impl_direct_image_fetch {
    ($field:ident, $name:expr, $referer:expr $(,)?) => {
        fn get_image_bytes(
            &self,
            url: ::std::string::String,
        ) -> $crate::anyhow::Result<$crate::bytes::Bytes> {
            $crate::fetch_direct_image(&self.$field, $name, url, $referer, module_path!())
        }
    };
}
