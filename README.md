# ShieldBlaze ExpressGateway
#### ShieldBlaze ExpressGateway is a High-Performance, Scalable, and Highly-Available Load Balancer.

![Build](https://github.com/shieldblaze/ExpressGateway/workflows/Java%20CI%20with%20Maven/badge.svg)
[![Total alerts](https://img.shields.io/lgtm/alerts/g/shieldblaze/ExpressGateway.svg?logo=lgtm&logoWidth=18)](https://lgtm.com/projects/g/shieldblaze/ExpressGateway/alerts/)
[![Language grade: Java](https://img.shields.io/lgtm/grade/java/g/shieldblaze/ExpressGateway.svg?logo=lgtm&logoWidth=18)](https://lgtm.com/projects/g/shieldblaze/ExpressGateway/context:java)
[![codecov](https://codecov.io/gh/shieldblaze/ExpressGateway/branch/main/graph/badge.svg?token=3TWQDLKVAA)](https://codecov.io/gh/shieldblaze/ExpressGateway)


#### Current Status: In-development | Last Call

## Features:
### L4:
- :heavy_check_mark: Load Balancing for TCP/UDP
- :heavy_check_mark: Full IPv6 Support

### L7:
- :heavy_check_mark: HTTP/1.1 and HTTP/2
- :heavy_check_mark: HTTP/2 to HTTP/1.1 Translation
- :heavy_check_mark: HTTP/1.1 to HTTP/2 Translation
- :heavy_check_mark: Reverse Proxy
- :heavy_check_mark: HTTP Compression (GZIP, Deflate and Brotli)
- :heavy_check_mark: HTTP Connection Pool
- :heavy_check_mark: WebSocket
- [ ] HTTP Caching

### TLS:
- :heavy_check_mark: TLS Support (v1.1, v1.2 and v1.3)
- :heavy_check_mark: TLS Offload
- :heavy_check_mark: Mutual TLS
- :heavy_check_mark: OCSP Stapling
- :heavy_check_mark: OCSP Certificate Validation
- :heavy_check_mark: Server Name Indication (SNI) Support
- :heavy_check_mark: StartTLS Support

### Health Checking:
- :heavy_check_mark: L4 Based Health Check using TCP/UDP
- :heavy_check_mark: L7 Based Health Check using HTTP/HTTPS

### Load Balancing Methods:
#### L4:
- :heavy_check_mark: Random
- :heavy_check_mark: Round Robin
- :heavy_check_mark: Least Connection
- :heavy_check_mark: Least Load

#### L7:
- :heavy_check_mark: HTTP Random
- :heavy_check_mark: HTTP Round Robin

### Session Persistence:
- :heavy_check_mark: 5-Tuple Hash (Source IP + Source Port +  Destination IP +  Destination Port + Protocol)
- :heavy_check_mark: Source IP Hash
- :heavy_check_mark: Long-Lived Sessions
- :heavy_check_mark: HTTP Cookie

### Security:
- :heavy_check_mark: Access Control List (ACL)
- :heavy_check_mark: Per-Connection Rate-Limit
- :heavy_check_mark: Per-Packet Rate-Limit
- [ ] Web Application Firewall (WAF)

### Cloud Integration:
- :heavy_check_mark: Amazon Web Services (AWS)
- [ ] Microsoft Azure (Azure)
- [ ] Google Cloud Platform (GCP)
