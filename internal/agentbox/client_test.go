package agentbox

import (
	"context"
	"errors"
	"reflect"
	"strings"
	"testing"
)

type fakeRunner struct {
	outputs [][]byte
	errors  []error
	calls   [][]string
}

func (runner *fakeRunner) CombinedOutput(_ context.Context, name string, args ...string) ([]byte, error) {
	call := append([]string{name}, args...)
	runner.calls = append(runner.calls, call)
	index := len(runner.calls) - 1
	return runner.outputs[index], runner.errors[index]
}

func TestShareBundleCreatesAttachesAndShares(t *testing.T) {
	runner := &fakeRunner{
		outputs: [][]byte{
			[]byte(`{"thread":{"id":"thr_example"}}`),
			[]byte(`{"message":{"id":"msg_example"}}`),
			[]byte(`{"visibility":{"teams":["ama"]}}`),
		},
		errors: make([]error, 3),
	}
	client := NewWithRunner(runner)
	id, err := client.ShareBundle(context.Background(), ShareRequest{
		Title:       "Shared skills",
		Team:        "ama",
		Message:     "Two skills",
		ArchivePath: "/tmp/skills.tar.gz",
	})
	if err != nil {
		t.Fatal(err)
	}
	if id != "thr_example" {
		t.Fatalf("thread ID = %q", id)
	}
	commands := []string{runner.calls[0][1], runner.calls[1][1], runner.calls[2][1]}
	if !reflect.DeepEqual(commands, []string{"create", "post", "visibility"}) {
		t.Fatalf("commands = %v", commands)
	}
	if got := strings.Join(runner.calls[2], " "); !strings.Contains(got, "--share-team ama") {
		t.Fatalf("visibility call = %q", got)
	}
}

func TestShareBundleReportsThreadOnAttachmentFailure(t *testing.T) {
	runner := &fakeRunner{
		outputs: [][]byte{[]byte(`{"id":"thr_example"}`), []byte("upload failed")},
		errors:  []error{nil, errors.New("exit 1")},
	}
	client := NewWithRunner(runner)
	_, err := client.ShareBundle(context.Background(), ShareRequest{
		Title: "Shared skills", Team: "ama", Message: "One skill", ArchivePath: "/tmp/skills.tar.gz",
	})
	if err == nil || !strings.Contains(err.Error(), "thr_example") {
		t.Fatalf("expected thread ID in error, got %v", err)
	}
}
