//! Personal input method is initialized lazily and cannot block console login.
#[derive(Default)]
pub struct LazyIme {
    engine: Option<crate::rime::Rime>,
}
impl LazyIme {
    pub fn ready(&self) -> bool {
        self.engine.is_some()
    }
    pub fn activate(&mut self, personal: bool) -> Result<(), String> {
        self.activate_with(personal, || {
            crate::config::open_rime(None).map_err(|e| e.to_string())
        })
    }
    fn activate_with(
        &mut self,
        personal: bool,
        create: impl FnOnce() -> Result<crate::rime::Rime, String>,
    ) -> Result<(), String> {
        if !personal {
            return Err("Personal IME is disabled before authentication".into());
        }
        if self.engine.is_none() {
            self.engine = Some(create()?);
        }
        Ok(())
    }
    pub fn clear(&mut self) {
        if let Some(engine) = &mut self.engine {
            engine.clear();
        }
    }
    pub fn process_key(&mut self, key: i32, mods: i32) -> Option<crate::rime::Update> {
        self.engine.as_mut().map(|e| e.process_key(key, mods))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn greeter_never_runs_initializer() {
        let mut ime = LazyIme::default();
        assert!(
            ime.activate_with(false, || panic!("user code executed"))
                .is_err()
        );
        assert!(!ime.ready());
    }
    #[test]
    fn initialization_failure_is_nonfatal_and_retryable() {
        let mut ime = LazyIme::default();
        assert!(ime.activate_with(true, || Err("locked".into())).is_err());
        ime.clear();
        assert!(!ime.ready());
        assert!(ime.activate_with(true, || Err("missing".into())).is_err());
    }
}
