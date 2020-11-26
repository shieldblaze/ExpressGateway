/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.healthcheck.l7;

import java.io.BufferedWriter;
import java.io.OutputStreamWriter;
import java.io.PrintWriter;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;

final class HTTPServer extends Thread{

    private final static String CRLF = "\r\n";
    private final String responseStatus;
    private final int port;

    public HTTPServer(String responseStatus, int port) {
        this.responseStatus = responseStatus;
        this.port = port;
    }

    @Override
    public void run() {
        try (ServerSocket serverSocket = new ServerSocket(port, 1000, InetAddress.getByName("127.0.0.1"))) {
            Socket clientSocket = serverSocket.accept();
            PrintWriter out = new PrintWriter(new BufferedWriter(new OutputStreamWriter(clientSocket.getOutputStream())),true);

            out.write("HTTP/1.1 " + responseStatus + CRLF);
            out.write("Server: Apache/0.8.4"+ CRLF);
            out.write("Content-Type: text/html"+ CRLF);
            out.write("Content-Length: 17"+ CRLF);
            out.write(CRLF);
            out.write("<TITLE>OK</TITLE>"+ CRLF);
            out.flush();
        } catch (Exception ex) {
            ex.printStackTrace();
        }
    }
}
