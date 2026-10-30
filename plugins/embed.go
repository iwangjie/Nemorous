// Package plugins holds the official plugin packages shipped inside
// Nemorous and the marketplace index they are listed in.
package plugins

import "embed"

// Builtin is the embedded copy of the official marketplace index and
// plugin packages. The app installs and uninstalls them like any other
// plugin, so uninstalling one leaves no surprise when it is reinstalled.
//
//go:embed registry.json maven
var Builtin embed.FS
