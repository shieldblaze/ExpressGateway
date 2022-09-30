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

    static void validateEnforce(boolean enforceConfigurationFileData, String componentMessage) {
        if (enforceConfigurationFileData) {
            System.err.println(componentMessage + " configuration was not found in configuration file");
            System.exit(1);
        }
    }

    static void checkNullOrEmptyEnv(String str, String componentMessage) {
        // If environment variable is 'null' or empty then we will throw error and shutdown.
        if (StringUtil.checkNullOrEmpty(str)) {
            System.err.println(componentMessage + " is not configured in Property/Environment Variable");
            System.exit(1);
        }
    }

    static void checkNullEnv(Object object, String componentMessage) {
        if (object == null) {
            System.err.println(componentMessage + " is not configured in Property/Environment Variable");
            System.exit(1);
        }
    }

    static String checkNullOrEmptyConf(JsonElement jsonElement, String componentMessage) {
        if (jsonElement.isJsonNull() || StringUtil.checkNullOrEmpty(jsonElement.getAsString())) {
            System.err.println(componentMessage + " is not configured in configuration file");
            System.exit(1);
            return null; // We will never reach here
        } else {
            return jsonElement.getAsString();
        }
    }

    private Utils() {
        // Prevent outside initialization
    }
}
