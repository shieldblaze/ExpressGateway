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
package com.shieldblaze.expressgateway.common.utils;

import java.net.InetAddress;

public final class NetworkUtil {

    /**
     * Check if a given port is valid
     *
     * @param port Port to check
     * @return {@code true} if port is valid else {@code false}
     */
    public static boolean isPortValid(int port) {
        try {
            NumberUtil.checkInRange(port, 1, 65535, "Port");
            return true;
        } catch (Exception ex) {
            return false;
        }
    }

    /**
     * Check if a given address is valid
     *
     * @param address Address (IPv4/6) to check
     * @return {@code true} if port is valid else {@code false}
     */
    public static boolean isAddressValid(String address) {
        try {
            InetAddress.getByName(address);
            return true;
        } catch (Exception ex) {
            return false;
        }
    }

    /**
     * Check if a given address and port is valid
     *
     * @param address Address to check
     * @param port    Port to check
     * @throws IllegalArgumentException Thrown if address or port is not valid
     */
    public static void checkAddressAndPort(String address, int port) throws IllegalArgumentException {
        if (!(isAddressValid(address) && isPortValid(port))) {
            throw new IllegalArgumentException("Invalid Address or Port");
        }
    }

    private NetworkUtil() {
        // Prevent outside initialization
    }
}
