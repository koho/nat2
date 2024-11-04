# nat2

nat2 simply means "nat to". It is a tool that helps you expose local services to the Internet in a Full-Cone NAT
network. Moreover, you can bind the mapped address automatically to your DNS provider or send it via HTTP request.

## Features

* Maintain the mapped address automatically.
* Bind the mapped address and port to DNS record.
* Support both TCP and UDP.
* HTTP hook.
* UPnP.

## Mapping

A mapping consists of two parts. The first part is called local endpoint url. The url scheme must be one
of `tcp`, `udp`, `tcp+upnp`, or `udp+upnp`.
The url host is an IPv4 address and the port must be present. For example, a valid endpoint url is `tcp://0.0.0.0:6666`.
The usage of this address:port pair is depend on UPnP state. When UPnP is active, the address:port pair is called the
forwarding address, which is the address of the local service that you want to open to the Internet.
When UPnP is disabled, the address:port pair is called the listen address. You must manually add port forwarding rules
in the gateway for the mapping to work properly.
The second part is called the watcher list, which is a list of tasks to be executed when the mapping is opened. Each
watcher is configured by the following JSON object.

| Field    | Type   | Description                                                                                                |
|----------|--------|------------------------------------------------------------------------------------------------------------|
| name     | string | The existing name of the watcher.                                                                          |
| value    | string | Value could contain placeholder `{ip}` and `{port}` which will be replaced with real value in the watcher. |
| domain   | string | Domain name.                                                                                               |
| type     | string | Record type.                                                                                               |
| priority | int    | Record priority. This field is required for record type SVCB, HTTPS and MX.                                |
| rid      | string | DNS record id. This field disables the automatic creation of dns records.                                  |
| ttl      | int    | TTL to use for dns records.                                                                                |
| proxied  | bool   | Whether the record is proxied by Cloudflare.                                                               |

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
    ]
  }
}
```

## Watcher

A watcher watches the update of mapped address. The watcher get notified when the mapped address is updated, and then it
can perform specific task.
You can define multiple watchers at the same time, just give them different name.

### DNSPod

DNSPod is a managed DNS providers. You can bind your mapped address to DNS record automatically using your secret id and
secret key from DNSPod.

```json
{
  "dnspod": {
    "personal": {
      "secret_id": "",
      "secret_key": ""
    },
    "company": {
      "secret_id": "",
      "secret_key": ""
    }
  }
}
```

### AliDNS

Alibaba DNS is a managed DNS providers.

| Field      | Type   | Description                                                                                                         |
|------------|--------|---------------------------------------------------------------------------------------------------------------------|
| url        | string | The request URL may vary by region. See https://api.aliyun.com/product/Alidns. Default is https://dns.aliyuncs.com. |
| secret_id  | string | Similar to username.                                                                                                |
| secret_key | string | Similar to password.                                                                                                |

```json
{
  "alidns": {
    "example": {
      "secret_id": "",
      "secret_key": ""
    }
  }
}
```

### Cloudflare

Cloudflare DNS is a managed DNS providers.

| Field | Type   | Description                                                                                 |
|-------|--------|---------------------------------------------------------------------------------------------|
| token | string | API token. See https://developers.cloudflare.com/fundamentals/api/get-started/create-token. |

```json
{
  "cf": {
    "example": {
      "token": ""
    }
  }
}
```

### HTTP

HTTP request is a common solution for sending event. The request is fully configurable.

| Field   | Type               | Description                                                                                                                                                        |
|---------|--------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| url     | string             | Request url could contain placeholder `{ip}` and `{port}` which will be replaced with real value before sending the request.                                       |
| method  | string             | Request method.                                                                                                                                                    |
| body    | string             | Request body could be JSON string, plain text, etc... Placeholder `{ip}` and `{port}` are supported. Note that this value could be overridden by watcher metadata. |
| headers | map<string,string> | Request headers. For example, `Content-Type` should be set based on the content in the `body`.                                                                     |

```json
{
  "http": {
    "api": {
      "url": "https://api.example.com",
      "method": "POST",
      "body": "{\"content\":\"{ip}:{port}\"}",
      "headers": {
        "Content-Type": "application/json; charset=utf-8"
      }
    }
  }
}
```

### Script

Run a script or program.

| Field | Type     | Description                                                                                                                                     |
|-------|----------|-------------------------------------------------------------------------------------------------------------------------------------------------|
| path  | string   | Path to executable file.                                                                                                                        |
| args  | []string | Arguments to pass to the program. If the `value` field in watcher metadata is not empty, it will be passed to the program as the last argument. |

For example, we have a python script named `test.py`:

```python
import sys
assert sys.argv[1] == "x.x.x.x:22274"
```

And our config file:

```json
{
  "map": {
    "udp://0.0.0.0:5555": [
      {
        "name": "example",
        "value": "{ip}:{port}"
      }
    ]
  },
  "script": {
    "example": {
      "path": "python",
      "args": [
        "test.py"
      ]
    }
  }
}
```

## Global options

### TCP mapping

| Field         | Type     | Description                                                                                                                                    |
|---------------|----------|------------------------------------------------------------------------------------------------------------------------------------------------|
| stun          | []string | TCP STUN server address:port pairs. The server must support STUN over TCP protocol. It selects hosts based on round-robin ordering.            |
| keepalive     | string   | Internet connectivity check url. Only HTTP protocol is supported. We will periodically fetch this url to maintain a long-lived TCP connection. |
| interval      | int      | The interval in seconds between fetching the keepalive url.                                                                                    |
| stun_interval | int      | The interval in seconds between sending binding request messages.                                                                              |

The following config is the default value:

```json
{
  "tcp": {
    "stun": [
      "turn.cloud-rtc.com:80"
    ],
    "keepalive": "http://www.baidu.com",
    "interval": 50,
    "stun_interval": 300
  }
}
```

### UDP mapping

| Field    | Type     | Description                                                                         |
|----------|----------|-------------------------------------------------------------------------------------|
| stun     | []string | UDP STUN server address:port pairs. It selects hosts based on round-robin ordering. |
| interval | int      | The interval in seconds between sending binding request messages.                   |

The following config is the default value:

```json
{
  "udp": {
    "stun": [
      "stun.chat.bilibili.com:3478",
      "stun.douyucdn.cn:18000",
      "stun.hitv.com:3478",
      "stun.miwifi.com:3478"
    ],
    "interval": 20
  }
}
```

### UPnP

Whether to use UPnP feature. Default is true. You can also use scheme `tcp+upnp://` or `udp+upnp://` to enable UPnP for
specific mapping.

The following config is the default value:

```json
{
  "upnp": true
}
```

If UPnP is not available in your local network. You must turn off this option and manually add port forwarding rules in
the gateway for the mapping to work properly. For example, we have a TCP mapping.

```json
{
  "map": {
    "tcp://0.0.0.0:50001": []
  },
  "upnp": false
}
```

In the gateway, add a port forwarding rule.

```shell
sudo iptables -t nat -A PREROUTING -i eth0 -p tcp --dport 50001 -j DNAT --to-destination 192.168.1.55:443
```

Now the endpoint 192.168.1.55:443 should be accessible via public mapped address.

## Run

The default config file path is `config.json` in the current directory. You can also use
argument `-c /path/to/your/config` to specify the config file path.

For example, run the service with the following command:

```shell
nat2 -c config.json
```

If you want to see more logs, turn on debug mode using `--debug` flag.

```shell
nat2 --debug -c config.json
```

## Lookup domain

Your can find your mapped address using `dig` or https://www.nslookup.io/svcb-lookup/.
