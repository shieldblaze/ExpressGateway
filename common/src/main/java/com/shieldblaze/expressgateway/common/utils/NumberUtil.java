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
package com.shieldblaze.expressgateway.common.utils;

public final class NumberUtil {

    public static int checkZeroOrPositive(int i, String message) {
        if (i >= 0) {
            return i;
        }
        throw new IllegalArgumentException(message + "; (Expected: " + i + " >= 0)");
    }

    public static long checkZeroOrPositive(long l, String message) {
        if (l >= 0) {
            return l;
        }
        throw new IllegalArgumentException(message + "; (Expected: " + l + " >= 0)");
    }

    public static int checkPositive(int i, String message) {
        if (i > 0) {
            return i;
        }
        throw new IllegalArgumentException(message + "; (Expected: " + i + " > 0)");
    }

    public static long checkPositive(long l, String message) {
        if (l > 0) {
            return l;
        }
        throw new IllegalArgumentException(message + "; (Expected: " + l + " > 0)");
    }

    public static int checkRange(int i, int start, int end, String message) {
        if (i >= start && i <= end) {
            return i;
        }
        throw new IllegalArgumentException(message + ": " + i + "; (Expected: " + start + "-" + end + ")");
    }

    public static long checkRange(long l, long start, long end, String message) {
        if (l >= start && l <= end) {
            return l;
        }
        throw new IllegalArgumentException(message + ": " + l + "; (Expected: " + start + "-" + end + ")");
    }
}
