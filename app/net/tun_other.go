//go:build !android

package main

import (
	"errors"

	"github.com/tailscale/wireguard-go/tun"
)

// a desktop reaches the network through the proxies instead (browser.go)
func tunFromFD(int) (tun.Device, error) {
	return nil, errors.New("a network interface is the phone's way in")
}
