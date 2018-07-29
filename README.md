## docker-guard

This project aims to provide a restricted `docker.sock` that can be used
without fear inside of untrusted containers. This ensures that no secrets
are exposed to the containers that have access to `docker.sock`.

## Restriction rules

Client is restricted to the following commands (everything else is blocked):

`docker ps` - It only shows the ID, status, and when container is created.
Everything else is filtered out.

`docker inspect <id>` - It only shows: ID, name, state, network settings,
and white-listed environment variables. Everything else is filtered out.

`docker version` - This commands is not filtered since there are no critical
information.

`docker info` - This shows basic information about the docker daemon, such as:
how many containers exists, how many are running, memory limit, etc. You can
find a complete list in `info` function of [filters.rs].

## Usage

docker-guard creates a UNIX socket at `/var/run/docker-guard/docker.sock` and
forwards all the allowed requests to `/var/run/docker.sock`.

There are 3 ways to white-list environment variables:

* Using `-e` option. You can use it multiple times or you can use comma as
  delimiter.
* Using `ENV_WHITELIST` environment variable. You can use comma delimiter.
* Using YAML or TOML config file. You can set the list of variables in
  `env_whitelist`.

In the following example docker-guard adds in the white-list `VAR[1-7]` variables.

```sh
echo 'env_whitelist = [ "VAR1", "VAR2" ]' > config.toml
export ENV_WHITELIST=VAR3,VAR4
docker-guard -e VAR5,VAR6 -e VAR7 -c config.toml
```

#### Real-life example

The actual reason that I created this project is to use it with [nginx-proxy].
This is a quick way to use it:

```sh
docker-guard -e VIRTUAL_HOST -e VIRTUAL_PORT
docker run -d \
    -v /var/run/docker-guard/docker.sock:/tmp/docker.sock:ro \
    -p 80:80 -p 443:443 jwilder/nginx-proxy
```

## License

MIT


[filters.rs]: src/filters.rs
[nginx-proxy]: https://github.com/jwilder/nginx-proxy
