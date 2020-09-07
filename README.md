# ShieldBlaze ExpressGateway
#### ShieldBlaze ExpressGateway is a high-performance network virtual applicance.

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
- [ ] Round Robin
- [ ] Random
- [ ] Source IP Hash
- [ ] Least Connection
- [ ] Weighted Round Robin
- [ ] Weighted Least Connection
- [ ] Geographic Proximity

### Session Persistence:
- [ ] 5-Tuple Hash (Source IP + Source Port +  Destination IP +  Destination Port + Protocol) 
- [ ] TLS Session ID
- [ ] HTTP Cookie
