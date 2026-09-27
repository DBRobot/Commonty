# The metrics test: a box reports itself and the dashboard reads it.

# a oneshot with no RemainAfterExit is "inactive" once done; run it and look
box.succeed("systemctl start dd-facts.service")
box.succeed("grep -q 'dd_box_cpu_cores{box=\"box\"}' /var/lib/dd-facts/dd_box.prom")
box.wait_for_unit("prometheus-node-exporter.service")
box.wait_until_succeeds("curl -sf http://127.0.0.1:9100/metrics -o /tmp/out && grep -q dd_box_memory_bytes /tmp/out")
# the box works out where it is; the ids are hashes with the right shape
geo.wait_for_unit("nginx.service")
# nginx up is not the network up: ask again until the answer is live
box.wait_until_succeeds("systemctl start dd-locate.service && grep -qE 'dd_box_location\\{box=\"box\",site=\"s-[0-9a-f]{8}\",region=\"r-[0-9a-f]{8}\",source=\"live\"\\} 1' /var/lib/dd-facts/dd_location.prom", timeout=60)
# the same place gives the same ids again, and a box that cannot ask
# keeps what it had
first = box.succeed("cat /var/lib/dd-facts/location.json")
geo.succeed("systemctl stop nginx.service")
box.succeed("systemctl start dd-locate.service")
assert box.succeed("cat /var/lib/dd-facts/location.json").replace("live", "cached") == first.replace("live", "cached")
box.succeed("grep -q 'source=\"cached\"' /var/lib/dd-facts/dd_location.prom")
box.wait_for_unit("prometheus.service")
box.wait_until_succeeds("curl -sf 'http://127.0.0.1:9090/api/v1/query?query=dd_box_cpu_cores' -o /tmp/out && grep -q '\"value\"' /tmp/out")
