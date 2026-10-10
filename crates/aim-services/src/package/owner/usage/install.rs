//! Usage ownership for a private live installation generation.
impl super::Usage {
    pub(crate) fn for_install<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            historical: self.historical,
            times: names.into_iter().map(|name| {
                (name.into(), self.times.get(name).copied().unwrap_or([0; super::REASONS]))
            }).collect(),
        }
    }
}
