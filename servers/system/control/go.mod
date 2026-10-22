module commonty/control

go 1.26.3

require (
	github.com/coder/websocket v1.8.15
	github.com/hyperledger/SmartBFT v1.0.2
	google.golang.org/protobuf v1.36.12
)

require golang.org/x/sync v0.23.0 // indirect

// SmartBFT v1.0.2 with three upstream fixes no release carries yet, picked
// onto the tag unchanged: #700 (f836851, a node one sequence behind
// rejoins), #713 (e65e57d) and #724 (3536b13), the controller and view
// changer deadlocks. Upstream main needs go 1.26.7; drop this for the first
// release that has them (docs/dev/control-plane.md, "When consensus wedges")
replace github.com/hyperledger/SmartBFT => ./third_party/smartbft
