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
 * Get value of supplied key using {@link System#getProperty(String)}
 * or {@link System#getenv(String)}
 */
public final class SystemPropertyUtil {

    /**
     * @param key Key to look for
     */
    public static String getPropertyOrEnv(String key) {
        String value = System.getProperty(key);

        // If null then it was not found in Property
        if (value == null) {
            value = System.getenv(key);
        }

        return value;
    }

    /**
     * @param key Key to look for
     */
    public static String getPropertyOrEnv(String key, String def) {
        String value = System.getProperty(key, def);

        // If null then it was not found in Property
        if (value == null) {
            value = System.getenv(key);
        }

        if (value == null) {
            value = def;
        }

        return value;
    }

    /**
     * @param key Key to look for
     */
    public static int getPropertyOrEnvInt(String key) {
        try {
            return Integer.parseInt(getPropertyOrEnv(key));
        } catch (Exception ex) {
            return -1;
        }
    }

    /**
     * @param key Key to look for
     */
    public static int getPropertyOrEnvInt(String key, String def) {
        try {
            return Integer.parseInt(getPropertyOrEnv(key, def));
        } catch (Exception ex) {
            return -1;
        }
    }

    /**
     * @param key Key to look for
     */
    public static long getUsingPropertyOrEnvironmentLong(String key) {
        try {
            return Long.parseLong(getPropertyOrEnv(key));
        } catch (Exception ex) {
            return -1;
        }
    }

    /**
     * @param key Key to look for
     */
    public static long getUsingPropertyOrEnvironmentLong(String key, String def) {
        try {
            return Long.parseLong(getPropertyOrEnv(key, def));
        } catch (Exception ex) {
            return -1;
        }
    }

    /**
     * @param key Key to look for
     */
    public static boolean getPropertyOrEnvBoolean(String key) {
        try {
            return Boolean.parseBoolean(getPropertyOrEnv(key));
        } catch (Exception ex) {
            return false;
        }
    }

    /**
     * @param key Key to look for
     */
    public static boolean getPropertyOrEnvBoolean(String key, String def) {
        try {
            return Boolean.parseBoolean(getPropertyOrEnv(key, def));
        } catch (Exception ex) {
            return false;
        }
    }

    private SystemPropertyUtil() {
        // Prevent outside initialization
    }
}
