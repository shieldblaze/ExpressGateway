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
package com.shieldblaze.expressgateway.bootstrap;

import com.google.gson.JsonElement;
import com.shieldblaze.expressgateway.common.utils.StringUtil;

final class Utils {

    static void validateEnforcing(boolean enforceConfigurationFileData, String componentMessage) {
        if (enforceConfigurationFileData) {
            throw new IllegalArgumentException(componentMessage + " configuration was not found in configuration file");
        }
    }

    static void validateStringNullOrEmptyEnv(String str, String componentMessage) {
        // If environment variable is 'null' or empty then we will throw error.
        if (StringUtil.isNullOrEmpty(str)) {
            throw new IllegalArgumentException(componentMessage + " is not configured in Property/Environment Variable");
        }
    }

    static void validateNullEnv(Object object, String componentMessage) {
        if (object == null) {
            throw new NullPointerException(componentMessage + " is not configured in Property/Environment Variable");
        }
    }

    static void validateJsonElementNullConf(JsonElement jsonElement, String componentMessage) {
        if (jsonElement == null || jsonElement.isJsonNull()) {
            throw new NullPointerException(componentMessage + " is not configured in configuration file");
        }
    }

    static boolean checkStringNullOrEmpty(JsonElement jsonElement) {
        return jsonElement == null || jsonElement.isJsonNull() || StringUtil.isNullOrEmpty(jsonElement.getAsString());
    }

    static String validateStringNullOrEmptyConf(JsonElement jsonElement, String componentMessage) {
        validateJsonElementNullConf(jsonElement, componentMessage);
        if (StringUtil.isNullOrEmpty(jsonElement.getAsString())) {
            throw new IllegalArgumentException(componentMessage + " is not configured in configuration file");
        } else {
            return jsonElement.getAsString();
        }
    }

    private Utils() {
        // Prevent outside initialization
    }
}
