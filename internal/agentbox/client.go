package agentbox

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os/exec"
	"strings"
)

type Runner interface {
	CombinedOutput(ctx context.Context, name string, args ...string) ([]byte, error)
}

type execRunner struct{}

func (execRunner) CombinedOutput(ctx context.Context, name string, args ...string) ([]byte, error) {
	return exec.CommandContext(ctx, name, args...).CombinedOutput()
}

type Client struct {
	runner Runner
}

type ShareRequest struct {
	Title       string
	Team        string
	Message     string
	ArchivePath string
}

func New() *Client {
	return NewWithRunner(execRunner{})
}

func NewWithRunner(runner Runner) *Client {
	return &Client{runner: runner}
}

func (client *Client) ShareBundle(ctx context.Context, request ShareRequest) (string, error) {
	created, err := client.commandJSON(ctx, "create", request.Title, "--message", request.Message, "--markdown", "--json")
	if err != nil {
		return "", fmt.Errorf("create thread: %w", err)
	}
	threadID := findThreadID(created)
	if threadID == "" {
		return "", errors.New("create thread: Agentbox response did not contain a thread ID")
	}

	if _, err := client.commandJSON(ctx, "post", threadID, "Complete skill bundle attached.", "--asset", request.ArchivePath, "--markdown", "--json"); err != nil {
		return "", fmt.Errorf("attach bundle to %s: %w", threadID, err)
	}
	if _, err := client.commandJSON(ctx, "visibility", threadID, "--share-team", request.Team, "--json"); err != nil {
		return "", fmt.Errorf("share %s with team %q: %w", threadID, request.Team, err)
	}
	return threadID, nil
}

func (client *Client) commandJSON(ctx context.Context, args ...string) (any, error) {
	output, err := client.runner.CombinedOutput(ctx, "agentbox", args...)
	if err != nil {
		return nil, fmt.Errorf("%s: %w", strings.TrimSpace(string(output)), err)
	}
	var result any
	if err := json.Unmarshal(output, &result); err != nil {
		return nil, fmt.Errorf("decode response: %w: %s", err, strings.TrimSpace(string(output)))
	}
	return result, nil
}

func findThreadID(value any) string {
	switch typed := value.(type) {
	case string:
		if strings.HasPrefix(typed, "thr_") {
			return typed
		}
	case []any:
		for _, item := range typed {
			if id := findThreadID(item); id != "" {
				return id
			}
		}
	case map[string]any:
		for _, key := range []string{"thread_id", "threadId", "id"} {
			if id := findThreadID(typed[key]); id != "" {
				return id
			}
		}
		for _, item := range typed {
			if id := findThreadID(item); id != "" {
				return id
			}
		}
	}
	return ""
}
