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

/**
 * Utility to sanitize user-controlled values before logging, preventing
 * CWE-117 log injection attacks by stripping CR/LF characters.
 * <p>
 * Because this method is not modeled in CodeQL's taint-tracking summaries,
 * it acts as a taint barrier — CodeQL will not propagate taint through it.
 */
public final class LogSanitizer {

    private LogSanitizer() {
    }

    /**
     * Sanitizes a string for safe inclusion in log messages by replacing
     * carriage return and newline characters with spaces.
     *
     * @param value the potentially tainted value
     * @return the sanitized value, or {@code "null"} if input is null
     */
    public static String sanitize(String value) {
        if (value == null) {
            return "null";
        }
        return value.replace('\r', ' ').replace('\n', ' ');
    }

    /**
     * Sanitizes a {@link CharSequence} for safe inclusion in log messages.
     *
     * @param value the potentially tainted value
     * @return the sanitized string, or {@code "null"} if input is null
     */
    public static String sanitize(CharSequence value) {
        if (value == null) {
            return "null";
        }
        return sanitize(value.toString());
    }
}
