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

import lombok.AccessLevel;
import lombok.NoArgsConstructor;

import java.util.regex.Pattern;

@NoArgsConstructor(access = AccessLevel.PRIVATE)
public final class Hostname {

    private static final Pattern COLON = Pattern.compile(":", Pattern.LITERAL);

    public static boolean doesHostAndPortMatch(String hostname, int port, String hostHeaderValue) {
        try {
            String[] valueSplit = COLON.split(hostHeaderValue, 3);

            return switch (valueSplit.length) {
                case 2 -> hostname.equalsIgnoreCase(valueSplit[0]) && checkPort(port) && port == Integer.parseInt(valueSplit[1]);
                case 1 -> hostname.equalsIgnoreCase(valueSplit[0]);
                default -> false;
            };
        } catch (Exception ex) {
            return false;
        }
    }

    private static boolean checkPort(int port) {
        return port > 0 && port <= 65535;
    }
}
