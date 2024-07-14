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

public final class NumberUtil {

    /**
     * Check if a given integer is zero (0) or a positive number.
     *
     * @param i       Integer to check
     * @param message Integer value name for constructing {@link IllegalArgumentException}
     * @return Integer itself
     */
    public static int checkZeroOrPositive(int i, String message) {
        if (i >= 0) {
            return i;
        }
        throw new IllegalArgumentException("Invalid " + message + "; (Expected: " + i + " >= 0)");
    }

    /**
     * Check if a given Long is zero (0) or a positive number.
     *
     * @param l       Long to check
     * @param message Long value name for constructing {@link IllegalArgumentException}
     * @return Long itself
     */
    public static long checkZeroOrPositive(long l, String message) {
        if (l >= 0) {
            return l;
        }
        throw new IllegalArgumentException(message + "; (Expected: " + l + " >= 0)");
    }

    /**
     * Check if a given integer a positive number.
     *
     * @param i       Integer to check
     * @param message Integer value name for constructing {@link IllegalArgumentException}
     * @return Integer itself
     */
    public static int checkPositive(int i, String message) {
        if (i > 0) {
            return i;
        }
        throw new IllegalArgumentException(message + "; (Expected: " + i + " > 0)");
    }

    /**
     * Check if a given Long a positive number.
     *
     * @param l       Long to check
     * @param message Long value name for constructing {@link IllegalArgumentException}
     * @return Long itself
     */
    public static long checkPositive(long l, String message) {
        if (l > 0) {
            return l;
        }
        throw new IllegalArgumentException(message + "; (Expected: " + l + " > 0)");
    }

    /**
     * Check if a Integer lies between a range
     *
     * @param i       Integer to check
     * @param start   Start of range
     * @param end     End of range
     * @param message Integer value name for constructing {@link IllegalArgumentException}
     * @return Integer itself
     */
    public static int checkInRange(int i, int start, int end, String message) {
        if (i >= start && i <= end) {
            return i;
        }
        throw new IllegalArgumentException("Invalid " + message + ": " + i + "; (Expected: " + start + '-' + end + ')');
    }

    /**
     * Check if a Long lies between a range
     *
     * @param l       Long to check
     * @param start   Start of range
     * @param end     End of range
     * @param message Long value name for constructing {@link IllegalArgumentException}
     * @return Long itself
     */
    public static long checkInRange(long l, long start, long end, String message) {
        if (l >= start && l <= end) {
            return l;
        }

        throw new IllegalArgumentException("Invalid " + message + ": " + l + "; (Expected: " + start + '-' + end + ')');
    }

    /**
     * Check if a Double lies between a range
     *
     * @param d       Long to check
     * @param start   Start of range
     * @param end     End of range
     * @param message Double value name for constructing {@link IllegalArgumentException}
     * @return Double itself
     */
    public static double checkInRange(double d, double start, double end, String message) {
        if (d >= start && d <= end) {
            return d;
        }
        throw new IllegalArgumentException("Invalid " + message + ": " + d + "; (Expected: " + start + '-' + end + ')');
    }

    /**
     * Check if a Float lies between a range
     *
     * @param f       Long to check
     * @param start   Start of range
     * @param end     End of range
     * @param message Float value name for constructing {@link IllegalArgumentException}
     * @return Float itself
     */
    public static float checkInRange(float f, float start, float end, String message) {
        if (f >= start && f <= end) {
            return f;
        }
        throw new IllegalArgumentException("Invalid " + message + ": " + f + "; (Expected: " + start + '-' + end + ')');
    }

    private NumberUtil() {
        // Prevent outside initialization
    }
}
