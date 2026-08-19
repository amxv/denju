package buildinfo

import "strings"

const defaultVersion = "dev"

// Version is overridden at build time via linker flags.
var Version = defaultVersion

func CurrentVersion() string {
	trimmed := strings.TrimSpace(Version)
	if trimmed == "" {
		return defaultVersion
	}
	return trimmed
}
