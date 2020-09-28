# ShieldBlaze ExpressGateway
#### ShieldBlaze ExpressGateway is a High-Performance Virtual Network Appliance.

![Java CI with Maven](https://github.com/shieldblaze/ExpressGateway/workflows/Java%20CI%20with%20Maven/badge.svg)
[![Total alerts](https://img.shields.io/lgtm/alerts/g/shieldblaze/ExpressGateway.svg?logo=lgtm&logoWidth=18)](https://lgtm.com/projects/g/shieldblaze/ExpressGateway/alerts/)
[![Language grade: Java](https://img.shields.io/lgtm/grade/java/g/shieldblaze/ExpressGateway.svg?logo=lgtm&logoWidth=18)](https://lgtm.com/projects/g/shieldblaze/ExpressGateway/context:java)


#### Current Status: In-development

## Features:
### L4:
- [X] Load Balancing for TCP/UDP
- [X] Full IPv6 Support
- [X] NAT-forwarding

### TLS:
- [X] TLS Support (v1.1, v1.2 and v1.3)
- [X] TLS Offload
- [X] Mutual TLS
- [X] OCSP Stapling
- [X] OCSP Certificate Validation
- [X] Server Name Indication (SNI) Support
- [X] StartTLS Support

### L7:
- [ ] HTTP/2
- [ ] Reverse Proxy
- [ ] HTTP Compression and Caching

### Health Checking:
- [X] L4 Based Health Check using TCP/UDP
- [X] L7 Based Health Check using HTTP/HTTPS

### Load Balancing Methods:
- [X] Random
- [X] Round Robin
- [X] Source IP Hash
- [X] Least Connection
- [X] Weighted Random
- [X] Weighted Round Robin
- [X] Weighted Least Connection

### Session Persistence:
- [X] 5-Tuple Hash (Source IP + Source Port +  Destination IP +  Destination Port + Protocol) 
- [X] Long-Lived Sessions
- [ ] HTTP Cookie

### Security:
- [ ] Access Control List (ACL)
- [ ] Rate-Limit
- [ ] Web Application Firewall (WAF)
