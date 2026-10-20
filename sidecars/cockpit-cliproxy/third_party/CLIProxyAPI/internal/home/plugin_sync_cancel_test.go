package home

import (
	"context"
	"errors"
	"io"
	"net"
	"sync"
	"testing"
	"time"
)

// Signal after the cancellation action has reached the underlying connection,
// so the later read deadline deterministically follows it instead of racing it.
type observedPluginSyncCancelConn struct {
	net.Conn
	applied chan struct{}
	once    sync.Once
}

func (c *observedPluginSyncCancelConn) SetDeadline(deadline time.Time) error {
	err := c.Conn.SetDeadline(deadline)
	c.once.Do(func() { close(c.applied) })
	return err
}

func (c *observedPluginSyncCancelConn) Close() error {
	err := c.Conn.Close()
	c.once.Do(func() { close(c.applied) })
	return err
}

func TestPluginSyncCancellationCannotBeUndoneByLaterReadDeadline(t *testing.T) {
	client, server := net.Pipe()
	t.Cleanup(func() { _ = client.Close(); _ = server.Close() })
	observed := &observedPluginSyncCancelConn{Conn: client, applied: make(chan struct{})}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	conn := newPluginSyncCancelableConn(ctx, observed)
	defer func() { _ = conn.Close() }()

	cancel()
	select {
	case <-observed.applied:
	case <-time.After(time.Second):
		t.Fatal("cancellation did not reach the dedicated connection")
	}

	// Redis may set its operation deadline after the cancellation watcher runs.
	// Cancellation must be permanent, not revert to waiting for this deadline.
	_ = conn.SetReadDeadline(time.Now().Add(100 * time.Millisecond))
	_, err := conn.Read(make([]byte, 1))
	if !errors.Is(err, net.ErrClosed) && !errors.Is(err, io.ErrClosedPipe) {
		t.Fatalf("read after cancellation = %v, want a closed connection", err)
	}
}
