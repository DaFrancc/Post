// This app's version, set when a release is built
// (`flutter build linux --dart-define=POST_VERSION=1.2.0`; see
// .github/workflows/release.yml). A build made without one is a
// development build.

/// The version this app was released as.
const appVersion = String.fromEnvironment('POST_VERSION', defaultValue: '0.0.0');

/// Built from source, not released: it does not look for updates by itself.
const isDevelopmentBuild = appVersion == '0.0.0';
