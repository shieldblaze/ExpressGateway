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

public final class StringUtil {

    public static final String EMPTY_STRING = "";

    /**
     * Return {@link Boolean#TRUE} if {@link String} is {@code null}, empty or blank
     * else returns {@link Boolean#FALSE}
     */
    public static boolean isNullOrEmpty(String str) {
        return str == null || str.isEmpty() || str.isBlank();
    }

    /**
     * Validate if a {@link String} is not null or empty
     *
     * @param str {@link String} to validate
     * @return Returns {@link String} if validation was successful
     * @throws NullPointerException     If {@link String} is null
     * @throws IllegalArgumentException If {@link String} is empty or black
     */
    public static String validateNotNullOrEmpty(String str) {
        if (str == null) {
            throw new NullPointerException("String is 'null'");
        } else if (str.isEmpty() || str.isBlank()) {
            throw new IllegalArgumentException("String is empty or blank");
        }
        return str;
    }

    /**
     * Validate if a {@link String} is not null or empty
     *
     * @param str  {@link String} to validate
     * @param name Name of {@link String} for logging in exception
     * @return Returns {@link String} if validation was successful
     * @throws NullPointerException     If {@link String} is null
     * @throws IllegalArgumentException If {@link String} is empty or black
     */
    public static String validateNotNullOrEmpty(String str, String name) {
        if (str == null) {
            throw new NullPointerException(name + " is 'null'");
        } else if (str.isEmpty() || str.isBlank()) {
            throw new IllegalArgumentException(name + " is empty or blank");
        }
        return str;
    }

    private StringUtil() {
        // Prevent outside initialization
    }
}
