# ShieldBlaze ExpressGateway
#### ShieldBlaze ExpressGateway is a High-Performance Virtual Network Appliance.

#### Current Status: In-development

## Features:
### L4:
- [ ] Load Balancing for TCP/UDP
- [ ] Full IPv6 Support
- [ ] NAT-forwarding

### L7:
- [ ] TLS Support (v1.0, v1.1, v1.2 and v1.3)
- [ ] TLS Offload
- [ ] HTTP/2
- [ ] Reverse Proxy
- [ ] HTTP Compression and Caching

### Health Checking:
- [ ] L4 Based Health Check using TCP/UDP
- [ ] L7 Based Health Check using HTTP/HTTPS

### Load Balancing Methods:
- [X] Round Robin
- [X] Random
- [X] Source IP Hash
- [X] Least Connection
- [X] Weighted Random
- [X] Weighted Round Robin
- [ ] Weighted Least Connection

### Session Persistence:
- [ ] 5-Tuple Hash (Source IP + Source Port +  Destination IP +  Destination Port + Protocol) 
- [ ] TLS Session ID
- [ ] HTTP Cookie
