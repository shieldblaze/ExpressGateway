/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2024 ShieldBlaze
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

package com.shieldblaze.expressgateway.concurrent.task;

import java.io.PrintWriter;
import java.io.StringWriter;

public class DefaultTaskError implements TaskError {
    private final StringBuilder message = new StringBuilder();
    private final StringBuilder stackTrace = new StringBuilder();

    @Override
    public void addThrowable(Throwable throwable) {
        if (throwable == null) {
            return;
        }
        message.append(throwable.getMessage()).append('\n');
        StringWriter sw = new StringWriter();
        throwable.printStackTrace(new PrintWriter(sw));
        stackTrace.append(sw).append('\n');
    }

    @Override
    public void addMessage(String message) {
        this.message.append(message).append('\n');
    }

    @Override
    public String toString() {
        return message.toString() + stackTrace;
    }
}
