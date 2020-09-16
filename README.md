# ShieldBlaze ExpressGateway
#### ShieldBlaze ExpressGateway is a High-Performance Virtual Network Appliance.

![Java CI with Maven](https://github.com/shieldblaze/ExpressGateway/workflows/Java%20CI%20with%20Maven/badge.svg)


#### Current Status: In-development

## Features:
### L4:
- [X] Load Balancing for TCP/UDP
- [X] Full IPv6 Support
- [X] NAT-forwarding

### L7:
- [ ] TLS Support (v1.0, v1.1, v1.2 and v1.3)
- [ ] TLS Offload
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
- [ ] TLS Session ID
- [ ] HTTP Cookie
