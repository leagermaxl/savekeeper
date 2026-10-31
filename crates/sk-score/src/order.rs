//! Category display order of the result (SPEC-09 §4.8).

use sk_core::model::Category;

/// Rank of a category in the result order of SPEC-09 §4.8: Credentials,
/// GameSave, UserFiles, DevEnvironment, AppData, AppConfig, GameConfig,
/// SystemSettings, Unknown, Reinstallable, Cache. Lower comes first.
///
/// This differs from the declaration order of [`Category`].
pub(crate) fn category_rank(category: Category) -> u8 {
    match category {
        Category::Credentials => 0,
        Category::GameSave => 1,
        Category::UserFiles => 2,
        Category::DevEnvironment => 3,
        Category::AppData => 4,
        Category::AppConfig => 5,
        Category::GameConfig => 6,
        Category::SystemSettings => 7,
        Category::Unknown => 8,
        Category::Reinstallable => 9,
        Category::Cache => 10,
    }
}
