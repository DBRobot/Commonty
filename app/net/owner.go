// Who may use the browser's way in: the account running the app, and no
// one else on the machine. Each connection is looked up in the system's
// own table of connections - which account opened it - and closed
// unanswered when that is not this one, or cannot be told.
package main

import "net"

// listenOwn listens on a free loopback port and accepts only this
// account's connections.
func listenOwn() (net.Listener, error) {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	return ownListener{l}, nil
}

type ownListener struct{ net.Listener }

func (l ownListener) Accept() (net.Conn, error) {
	for {
		c, err := l.Listener.Accept()
		if err != nil {
			return nil, err
		}
		client, ok1 := c.RemoteAddr().(*net.TCPAddr)
		server, ok2 := c.LocalAddr().(*net.TCPAddr)
		if ok1 && ok2 && sameAccount(client, server) {
			return c, nil
		}
		c.Close()
	}
}
