package app

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/amxv/agentbox-skill-share/internal/agentbox"
	"github.com/amxv/agentbox-skill-share/internal/buildinfo"
	"github.com/amxv/agentbox-skill-share/internal/bundle"
)

const commandName = "agentbox-skill-share"

var version = buildinfo.CurrentVersion()

type options struct {
	skillsDir   string
	team        string
	title       string
	archive     string
	packageOnly bool
}

func Run(args []string, stdout, stderr io.Writer) error {
	return run(context.Background(), args, stdout, stderr, agentbox.New())
}

func run(ctx context.Context, args []string, stdout, stderr io.Writer, client *agentbox.Client) error {
	if len(args) == 0 || isHelpArg(args[0]) {
		printRootHelp(stdout)
		return nil
	}
	if len(args) == 1 && args[0] == "--version" {
		_, _ = fmt.Fprintf(stdout, "%s %s\n", commandName, version)
		return nil
	}

	parsed, names, err := parseOptions(args, stderr)
	if err != nil {
		return err
	}
	if len(names) == 0 {
		return errors.New("provide at least one skill name")
	}
	if !parsed.packageOnly && strings.TrimSpace(parsed.team) == "" {
		return errors.New("--team is required unless --package-only is used")
	}

	root, err := expandHome(parsed.skillsDir)
	if err != nil {
		return err
	}
	names, err = bundle.ValidateSkills(root, names)
	if err != nil {
		return err
	}

	archivePath, cleanup, err := archiveDestination(parsed.archive, parsed.packageOnly)
	if err != nil {
		return err
	}
	defer cleanup()

	if err := bundle.Create(root, names, archivePath); err != nil {
		return fmt.Errorf("create archive: %w", err)
	}
	if parsed.packageOnly {
		_, _ = fmt.Fprintln(stdout, archivePath)
		return nil
	}

	if parsed.title == "" {
		parsed.title = fmt.Sprintf("Shared agent skills (%d)", len(names))
	}
	threadID, err := client.ShareBundle(ctx, agentbox.ShareRequest{
		Title:       parsed.title,
		Team:        parsed.team,
		Message:     bundleMessage(names, parsed.team),
		ArchivePath: archivePath,
	})
	if err != nil {
		return err
	}

	_, _ = fmt.Fprintf(stdout, "Created and shared Agentbox thread %s with team %s\n", threadID, parsed.team)
	return nil
}

func parseOptions(args []string, stderr io.Writer) (options, []string, error) {
	var parsed options
	flags := flag.NewFlagSet(commandName, flag.ContinueOnError)
	flags.SetOutput(stderr)
	flags.Usage = func() { printRootHelp(stderr) }
	flags.StringVar(&parsed.skillsDir, "skills-dir", "~/.agents/skills", "directory containing skill folders")
	flags.StringVar(&parsed.team, "team", "", "Agentbox team slug or ID")
	flags.StringVar(&parsed.title, "title", "", "thread title")
	flags.StringVar(&parsed.archive, "archive", "", "archive output path")
	flags.BoolVar(&parsed.packageOnly, "package-only", false, "create the archive without posting it")
	if err := flags.Parse(args); err != nil {
		return options{}, nil, err
	}
	return parsed, flags.Args(), nil
}

func expandHome(path string) (string, error) {
	if path == "~" || strings.HasPrefix(path, "~/") {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", fmt.Errorf("resolve home directory: %w", err)
		}
		if path == "~" {
			return home, nil
		}
		return filepath.Join(home, strings.TrimPrefix(path, "~/")), nil
	}
	return filepath.Abs(path)
}

func archiveDestination(requested string, packageOnly bool) (string, func(), error) {
	if requested != "" {
		path, err := filepath.Abs(requested)
		if err != nil {
			return "", func() {}, err
		}
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			return "", func() {}, err
		}
		return path, func() {}, nil
	}

	name := "agent-skills-" + time.Now().Format("20060102-150405") + ".tar.gz"
	if packageOnly {
		path, err := filepath.Abs(name)
		return path, func() {}, err
	}

	dir, err := os.MkdirTemp("", "agentbox-skills-")
	if err != nil {
		return "", func() {}, err
	}
	return filepath.Join(dir, name), func() { _ = os.RemoveAll(dir) }, nil
}

func bundleMessage(names []string, team string) string {
	var body strings.Builder
	fmt.Fprintf(&body, "Sharing %d reusable agent skills with the `%s` team:\n\n", len(names), team)
	for _, name := range names {
		fmt.Fprintf(&body, "- `%s`\n", name)
	}
	body.WriteString("\nThe attached `.tar.gz` contains each complete skill directory, including scripts and supporting files. Extract it into `~/.agents/skills/`.\n")
	return body.String()
}

func isHelpArg(value string) bool {
	switch value {
	case "-h", "--help", "help":
		return true
	default:
		return false
	}
}

func printRootHelp(w io.Writer) {
	writeLines(w,
		"agentbox-skill-share - package and share complete agent skills through Agentbox",
		"",
		"Usage:",
		"  agentbox-skill-share [options] <skill> [<skill> ...]",
		"  agentbox-skill-share --version",
		"",
		"Options:",
		"  --skills-dir <path>  skill directory root (default: ~/.agents/skills)",
		"  --team <slug>        Agentbox team to share with",
		"  --title <title>      thread title",
		"  --archive <path>     retain the bundle at this path",
		"  --package-only       create a bundle without using Agentbox",
		"",
		"Examples:",
		"  agentbox-skill-share --team ama agentbox dogfood",
		"  agentbox-skill-share --package-only --archive skills.tar.gz agentbox dogfood",
	)
}

func writeLines(w io.Writer, lines ...string) {
	for _, line := range lines {
		_, _ = fmt.Fprintln(w, line)
	}
}
