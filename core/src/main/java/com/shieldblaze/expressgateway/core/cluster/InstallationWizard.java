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
package com.shieldblaze.expressgateway.core.cluster;

import java.util.Scanner;

public final class InstallationWizard {

    public static void topHeader() {
        print("----------------------------------------------------------------------");
        print("=================== | ShieldBlaze ExpressGateway| ====================");
    }

    public static void bottomHeader() {
        print("======================================================================");
        print("----------------------------------------------------------------------");
    }

    public static void beginInstall() {
        try (Scanner scanner = new Scanner(System.in)) {

            print("Available options:");
            print("> installFresh");
            print("> installNode");

            String option = scanner.nextLine().toLowerCase();
            switch (option) {
                case "installFresh":
                    installFresh();
                    break;
                case "installNode":
                    break;
                default:
                    System.err.println("Unknown Option: " + option);
                    beginInstall();
            }
        } catch (Exception ex) {
            System.err.println("Error Occurred: " + ex.getMessage());
            System.exit(1);
        }
    }

    private static void installFresh() {
        print("====================| Installation Wizard |====================");


    }

    private static void print(String str) {
        System.out.println(str);
    }
}
