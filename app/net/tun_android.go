//go:build android

package main

import (
	"github.com/tailscale/wireguard-go/tun"
)

// the phone's VPN interface, by the descriptor Android handed the app
func tunFromFD(fd int) (tun.Device, error) {
	dev, _, err := tun.CreateUnmonitoredTUNFromFD(fd)
	return dev, err
}
