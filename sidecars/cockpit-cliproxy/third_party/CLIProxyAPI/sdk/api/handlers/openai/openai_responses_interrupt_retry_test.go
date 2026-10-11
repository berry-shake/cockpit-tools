package openai

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gorilla/websocket"
	"github.com/tidwall/gjson"
)

func TestResponsesRejectedInterruptReleasesPendingBeforeErrorReply(t *testing.T) {
	ready := make(chan *responsesWebsocketReadPump, 1)
	entered := make(chan struct{})
	retried := make(chan struct{})
	var calls atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		conn, err := responsesWebsocketUpgrader.Upgrade(w, r, nil)
		if err != nil {
			t.Error(err)
			return
		}
		local := newResponsesLocalInterrupt()
		local.begin(r.Context())
		defer local.end()
		local.observeResponseID([]byte(`{"type":"response.created","response":{"id":"r1"}}`))
		writer := newResponsesWebsocketWriter(conn)
		// Hold the error reply at the writer, not at a scheduler-dependent delay.
		writer.writeMu.Lock()
		pump := newResponsesWebsocketReadPump(r.Context(), conn, func(context.Context, []byte) error {
			if calls.Add(1) == 1 {
				close(entered)
				return errors.New("temporarily disabled")
			}
			close(retried)
			return nil
		}, local, writer)
		defer pump.stop()
		ready <- pump
		<-pump.ctx.Done()
	}))
	defer server.Close()
	client, _, err := websocket.DefaultDialer.Dial("ws"+strings.TrimPrefix(server.URL, "http"), nil)
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	pump := <-ready
	var unlock sync.Once
	releaseWriter := func() { unlock.Do(pump.writer.writeMu.Unlock) }
	defer releaseWriter()
	waitFor := func(message string, condition func() bool) {
		t.Helper()
		deadline := time.After(2 * time.Second)
		ticker := time.NewTicker(time.Millisecond)
		defer ticker.Stop()
		for !condition() {
			select {
			case <-deadline:
				t.Fatal(message)
			case <-ticker.C:
			}
		}
	}
	payload := []byte(`{"type":"response.interrupt","response_id":"r1"}`)
	if err := client.WriteMessage(websocket.TextMessage, payload); err != nil {
		t.Fatal(err)
	}
	select {
	case <-entered:
	case <-time.After(2 * time.Second):
		t.Fatal("interrupt did not start")
	}
	waitFor("rejected interrupt remains deduplicated while its error reply is being written", func() bool {
		pump.interruptMu.Lock()
		defer pump.interruptMu.Unlock()
		return len(pump.interruptPending) == 0 && pump.interruptBytes == 0
	})
	// Queue the identical retry before the first error write returns. Finishing
	// the old job must not remove this new job's pending entry or byte budget.
	if !pump.queueInterrupt(payload) {
		t.Fatal("retry was rejected")
	}
	if len(pump.interruptJobs) != 1 {
		t.Fatal("retry was incorrectly coalesced with the failed interrupt")
	}
	releaseWriter()
	_ = client.SetReadDeadline(time.Now().Add(2 * time.Second))
	_, reply, err := client.ReadMessage()
	if err != nil || gjson.GetBytes(reply, "type").String() != "error" {
		t.Fatalf("expected original error reply, got %s: %v", reply, err)
	}
	select {
	case <-retried:
	case <-time.After(2 * time.Second):
		t.Fatal("identical retry never reached the interrupt handler")
	}
	waitFor("retry leaked pending state or double-released the byte budget", func() bool {
		pump.interruptMu.Lock()
		defer pump.interruptMu.Unlock()
		return len(pump.interruptPending) == 0 && pump.interruptBytes == 0
	})
}
