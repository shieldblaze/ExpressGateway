/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.server.udp;

import java.net.Inet4Address;
import java.net.InetSocketAddress;
import java.util.Arrays;

final class AddressUtils {

    private AddressUtils() {
        // Prevent outside initialization
    }

    /**
     * Convert IP Address and Port into Byte Array
     */
    static byte[] address(InetSocketAddress socketAddress) {
        byte[] bytes;
        if (socketAddress.getAddress() instanceof Inet4Address) {
            bytes = new byte[6];
            for (int i = 0; i < 4; i++) {
                bytes[i] = socketAddress.getAddress().getAddress()[i];
            }

            short port = (short) socketAddress.getPort();

            bytes[4] = (byte) (port & 0xff);
            bytes[5] = (byte) ((port >> 8) & 0xff);
        } else {
            bytes = new byte[10];
            for (int i = 0; i < 4; i++) {
                bytes[i] = socketAddress.getAddress().getAddress()[i];
            }

            short port = (short) socketAddress.getPort();

            bytes[8] = (byte) (port & 0xff);
            bytes[9] = (byte) ((port >> 8) & 0xff);
        }
        return bytes;
    }
}
