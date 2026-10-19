//! # DI Scopes and Lifetime Management

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DependencyScope {
    /// Created once on application boot; shared across all workers and requests.
    Singleton,
    /// Created at most once per incoming request and stored in request arena.
    RequestScoped,
    /// Instantiated newly each time requested.
    Transient,
}
