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
package com.shieldblaze.expressgateway.intercommunication.messages.request;

import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.intercommunication.KeyValuePair;
import com.shieldblaze.expressgateway.intercommunication.messages.KeyPairAdapter;
import com.shieldblaze.expressgateway.intercommunication.messages.RequestMessage;
import io.netty.buffer.ByteBuf;

import java.util.List;

public final class DeleteDataRequest extends KeyPairAdapter implements RequestMessage {

    private final ByteBuf id;

    @NonNull
    public DeleteDataRequest(ByteBuf id, List<KeyValuePair> keyValuePairList) {
        super(keyValuePairList);
        this.id = id;
    }

    @Override
    public ByteBuf id() {
        return id;
    }
}
