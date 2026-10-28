# WABTY Studio branding

Updated 2026-10-27.

The product name is **WABTY Studio** and the desktop bundle/application ID is
**`com.wabty.studio`**. Native window titles, application menus, browser startup,
plugin host identity, About, desktop entries, Flatpak manifests and installer
metadata use that identity. Windows uses a distinct MSI upgrade code and component
GUIDs so this fork does not upgrade or remove an upstream installation.

Settings are separate from upstream installations:

- macOS: `~/Library/Application Support/WABTY Studio`
- Windows: `%APPDATA%\WABTY Studio`
- Linux/other Unix: `$XDG_CONFIG_HOME/wabty-studio` or `~/.config/wabty-studio`
- Portable: `WABTYStudioData` beside the executable

Existing settings are not migrated automatically. `SOUNDCRAFT_CONFIG_DIR` remains
an explicit override. Internal Cargo package and executable names, environment
variables, MCP resource URIs, session extensions and media MIME types retain their
existing names for compatibility. `.scraft` sessions remain readable.

## Release services

No WABTY repository or public support URL has been supplied. Upstream promotional,
support and download links have been removed from product surfaces. The application
has no automatic updater; Check for Updates explains that updates are unconfigured.
AppImage delta updates are off unless `WABTY_RELEASE_REPOSITORY=owner/repository` is
explicitly supplied during packaging. Upstream SonarCloud reporting is removed.
Signing and publishing require the distributor's own credentials.

The existing Cargo dependency sources and reusable GitHub Actions workflows still
reference their actual upstream repositories. They are build dependencies, not
WABTY support or update endpoints. Git remotes have not been changed. Historical
issue references in parity documents identify the original project intentionally.

## Attribution

WABTY Studio is based on SoundCraft by the ArtCraft Team and contributors. Original
copyrights, license texts, generated contributor credits, source provenance and
asset authors remain intact. NOTICE is available in the About window. Restricted
ArtCraft logo assets and the old branded DMG background/layout are removed; their
historical attribution and trademark terms are retained. The permissively licensed
original app icon remains, with its original authors credited in ATTRIBUTION.md.
