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
package com.shieldblaze.expressgateway.controlinterface.configuration;

import com.google.protobuf.ProtocolStringList;
import com.shieldblaze.expressgateway.configuration.tls.Cipher;
import com.shieldblaze.expressgateway.configuration.tls.Protocol;

import java.util.ArrayList;
import java.util.List;

final class Utils {

    static List<Protocol> protocolConverter(ProtocolStringList protocolStringList) {
        List<Protocol> protocols = new ArrayList<>();
        for (String protocol : protocolStringList) {
            protocols.add(Protocol.get(protocol));
        }
        return protocols;
    }

    static List<Cipher> cipherConverter(ProtocolStringList protocolStringList) {
        List<Cipher> ciphers = new ArrayList<>();
        for (String cipher : protocolStringList) {
            ciphers.add(Cipher.valueOf(cipher.toUpperCase()));
        }
        return ciphers;
    }
}
