/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.servicediscovery.client;

import lombok.extern.slf4j.Slf4j;

import javax.naming.NamingEnumeration;
import javax.naming.NamingException;
import javax.naming.directory.Attributes;
import javax.naming.directory.DirContext;
import javax.naming.directory.InitialDirContext;
import java.net.InetAddress;
import java.util.ArrayList;
import java.util.Hashtable;
import java.util.List;

/**
 * DNS-based service discovery as a fallback when the HTTP-based discovery server
 * is unreachable. Resolves SRV records first, falling back to A/AAAA records.
 *
 * <p>This is a last-resort discovery mechanism. The assumption is that a DNS name
 * (e.g., {@code _expressgateway._tcp.service.local}) resolves to active service instances.
 * SRV records provide port information; plain A/AAAA records use a default port.</p>
 */
@Slf4j
public final class DnsServiceDiscovery {

    private final String dnsName;
    private final int defaultPort;

    /**
     * @param dnsName     the DNS name to resolve (e.g., "expressgateway.service.consul" or SRV name)
     * @param defaultPort default port to use when SRV records are not available
     */
    public DnsServiceDiscovery(String dnsName, int defaultPort) {
        this.dnsName = dnsName;
        this.defaultPort = defaultPort;
    }

    /**
     * Resolve service entries from DNS. Tries SRV records first, then A/AAAA.
     *
     * @return list of discovered service entries (may be empty, never null)
     */
    public List<ServiceEntry> resolve() {
        List<ServiceEntry> entries = resolveSrv();
        if (!entries.isEmpty()) {
            return entries;
        }
        return resolveAddressRecords();
    }

    /**
     * Resolve SRV records. Each record provides host, port, and priority.
     */
    private List<ServiceEntry> resolveSrv() {
        List<ServiceEntry> entries = new ArrayList<>();
        DirContext ctx = null;
        try {
            Hashtable<String, String> env = new Hashtable<>();
            env.put("java.naming.factory.initial", "com.sun.jndi.dns.DnsContextFactory");
            ctx = new InitialDirContext(env);

            Attributes attrs = ctx.getAttributes(dnsName, new String[]{"SRV"});
            NamingEnumeration<?> srvRecords = attrs.get("SRV") != null ? attrs.get("SRV").getAll() : null;

            if (srvRecords != null) {
                while (srvRecords.hasMore()) {
                    String record = srvRecords.next().toString();
                    // SRV format: priority weight port target
                    String[] parts = record.split("\\s+");
                    if (parts.length >= 4) {
                        int port;
                        try {
                            port = Integer.parseInt(parts[2]);
                        } catch (NumberFormatException e) {
                            log.warn("Invalid port in SRV record '{}': {}", record, e.getMessage());
                            continue;
                        }
                        String host = parts[3];
                        // Remove trailing dot from DNS name
                        if (host.endsWith(".")) {
                            host = host.substring(0, host.length() - 1);
                        }
                        // Resolve the host to IP
                        try {
                            InetAddress addr = InetAddress.getByName(host);
                            entries.add(ServiceEntry.of(
                                    "dns-" + addr.getHostAddress() + "-" + port,
                                    addr.getHostAddress(), port, false));
                        } catch (Exception ex) {
                            log.warn("Failed to resolve SRV target host: {}", host, ex);
                        }
                    }
                }
            }
        } catch (NamingException ex) {
            log.debug("SRV lookup failed for {}: {}", dnsName, ex.getMessage());
        } finally {
            if (ctx != null) {
                try {
                    ctx.close();
                } catch (NamingException ignored) {
                }
            }
        }
        return entries;
    }

    /**
     * Resolve A/AAAA records and use the default port.
     */
    private List<ServiceEntry> resolveAddressRecords() {
        List<ServiceEntry> entries = new ArrayList<>();
        try {
            InetAddress[] addresses = InetAddress.getAllByName(dnsName);
            for (InetAddress addr : addresses) {
                entries.add(ServiceEntry.of(
                        "dns-" + addr.getHostAddress() + "-" + defaultPort,
                        addr.getHostAddress(), defaultPort, false));
            }
        } catch (Exception ex) {
            log.debug("DNS A/AAAA lookup failed for {}: {}", dnsName, ex.getMessage());
        }
        return entries;
    }
}
