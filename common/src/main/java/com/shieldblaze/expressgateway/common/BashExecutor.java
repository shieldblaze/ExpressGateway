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
package com.shieldblaze.expressgateway.common;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;

public final class BashExecutor {

    private static final Logger logger = LogManager.getLogger(BashExecutor.class);

    public static void execute(String bashScriptFileName) {
        ProcessBuilder processBuilder = new ProcessBuilder();
        processBuilder.command("bash", "-c", bashScriptFileName);

        BufferedReader normalStream = null;
        BufferedReader errorStream = null;
        try {
            Process process = processBuilder.start();

            normalStream = new BufferedReader(new InputStreamReader(process.getInputStream()));
            errorStream = new BufferedReader(new InputStreamReader(process.getErrorStream()));

            String line;
            while ((line = normalStream.readLine()) != null) {
                System.out.println(line);
            }

            while ((line = errorStream.readLine()) != null) {
                System.out.println(line);
            }
        } catch (IOException e) {
            logger.error("Error occurred while executing \"" + bashScriptFileName + "\"", e);
        } finally {
            try {
                if (normalStream != null) {
                    normalStream.close();
                }

                if (errorStream != null) {
                    errorStream.close();
                }
            } catch (Exception ex) {
                // Ignore
            }
        }
    }

    private BashExecutor() {
        // Prevent outside initialization
    }
}
