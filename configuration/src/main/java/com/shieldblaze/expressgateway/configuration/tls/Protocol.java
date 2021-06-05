/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

import java.util.List;
import java.util.NoSuchElementException;

/**
 * List of available TLS Protocols under OpenSsl 1.1.1h
 */
public enum Protocol {
    TLS_1_1("TLSv1.1"),
    TLS_1_2("TLSv1.2"),
    TLS_1_3("TLSv1.3");
    
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
            protocolArray[index] = p.protocol;
            index++;
        }
        return protocolArray;
    }

    public static Protocol get(String protocol) {
        if (protocol.equalsIgnoreCase("TLSv1.1")) {
            return TLS_1_1;
        } else if (protocol.equalsIgnoreCase("TLSv1.2")) {
            return TLS_1_2;
        } else if (protocol.equalsIgnoreCase("TLSv1.3")) {
            return TLS_1_3;
        }
       throw new NoSuchElementException("Invalid Protocol: " + protocol);
    }
}
