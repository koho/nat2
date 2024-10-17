# nat2

Expose your local service to Internet in NAT1 network.

* Maintain the mapped address automatically.
* Bind the mapped address and port to DNS record.
* Support both TCP and UDP.
* HTTP hook.
* UPnP.

An example config file:

```json
{
  "map": {
    "tcp://0.0.0.0:6666": [
      {
        "name": "ddns",
        "domain": "test.example.com",
        "type": "HTTPS",
        "value": ". alpn=\"h2\" ipv4hint=\"{ip}\" port=\"{port}\"",
        "priority": 1
      }
    ],
    "udp://0.0.0.0:2222": [
      {
        "name": "api",
        "value": ""
      }
    ]
  },
  "http": {
    "api": {
      "url": "https://api.example.com",
      "method": "POST",
      "body": "{\"content\":\"{ip}:{port}\"}",
      "headers": {
        "Content-Type": "application/json; charset=utf-8"
      }
    }
  },
  "dnspod": {
    "ddns": {
      "secret_id": "",
      "secret_key": ""
    }
  }
}
```

Run the service with the following command:

```shell
nat2 -c config.json
```

### Without UPnP

For each mapping, add corresponding forwarding rules to the router.

```shell
sudo iptables -t nat -A PREROUTING -i eth0 -p tcp --dport 50001 -j DNAT --to-destination 192.168.1.55:443
```

### Lookup domain

Your can find your mapped address using `dig` or https://www.nslookup.io/svcb-lookup/.
