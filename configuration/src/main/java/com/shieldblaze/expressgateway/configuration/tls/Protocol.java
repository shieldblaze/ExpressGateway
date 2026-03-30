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
package com.shieldblaze.expressgateway.configuration.tls;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.List;
import java.util.NoSuchElementException;

/**
 * List of available TLS Protocols under OpenSsl 1.1.1h
 */
public enum Protocol {
    /**
     * @deprecated TLS 1.1 is formally deprecated per RFC 8996. Must not be used in PRODUCTION environments.
     * Use {@link #TLS_1_2} or {@link #TLS_1_3} instead.
     */
    @Deprecated(since = "RFC 8996", forRemoval = true)
    TLS_1_1("TLSv1.1"),
    TLS_1_2("TLSv1.2"),
    TLS_1_3("TLSv1.3");

    private static final Logger logger = LogManager.getLogger(Protocol.class);

    private final String protocol;

    Protocol(String protocol) {
        this.protocol = protocol;
    }

    public String protocol() {
        return protocol;
    }

    static String[] getProtocols(List<Protocol> protocols) {
        String[] protocolArray = new String[protocols.size()];
        int index = 0;
        for (Protocol p : protocols) {
            // TLS-F4: Warn when deprecated TLS versions are selected. TLS 1.0 and 1.1
            // are formally deprecated by RFC 8996 due to known cryptographic weaknesses
            // (BEAST, POODLE, lack of AEAD ciphers). Operators should migrate to TLS 1.2+.
            if (p == TLS_1_1) {
                logger.warn("TLS 1.1 is deprecated per RFC 8996 and has known security vulnerabilities. " +
                        "Migrate to TLS 1.2 or TLS 1.3.");
            }
            protocolArray[index] = p.protocol;
            index++;
        }
        return protocolArray;
    }

    public static Protocol get(String protocol) {
        if ("TLSv1.1".equalsIgnoreCase(protocol)) {
            // TLS-F4: Warn at resolution time so config-file parsing surfaces the issue early.
            logger.warn("TLS 1.1 is deprecated per RFC 8996 and has known security vulnerabilities. " +
                    "Migrate to TLS 1.2 or TLS 1.3.");
            return TLS_1_1;
        }
        if ("TLSv1.2".equalsIgnoreCase(protocol)) {
            return TLS_1_2;
        }
        if ("TLSv1.3".equalsIgnoreCase(protocol)) {
            return TLS_1_3;
        }
        throw new NoSuchElementException("Invalid Protocol: " + protocol);
    }
}
