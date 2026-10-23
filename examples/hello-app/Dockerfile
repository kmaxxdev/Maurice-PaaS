# Tiny image used to exercise "service from a Dockerfile in a git repository".
FROM docker.io/library/nginx:1.27-alpine
COPY index.html /usr/share/nginx/html/index.html
HEALTHCHECK --interval=10s CMD wget -qO- http://127.0.0.1/ >/dev/null || exit 1
