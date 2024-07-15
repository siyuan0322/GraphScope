/**
 * Copyright 2020 Alibaba Group Holding Limited.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
package com.alibaba.graphscope.groot.dataload.databuild;

import org.rocksdb.*;

import java.io.IOException;
import java.nio.ByteBuffer;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.time.ZoneId;
import java.time.ZonedDateTime;
import java.util.Arrays;

public class SstRecordWriter {
    private final SstFileWriter sstFileWriter;
    private final String charSet;
    private boolean isEmpty;
    private final boolean ttlEnabled;
    private final byte[] midnightTS;

    public SstRecordWriter(String fileName, String charSet) throws IOException {
         this(fileName, charSet, false);
    }

    public static byte[] longToBytes(long x) {
        ByteBuffer buffer = ByteBuffer.allocate(Long.BYTES);
        buffer.putLong(x);
        return buffer.array();
    }

    public static long bytesToLong(byte[] bytes) {
        ByteBuffer buffer = ByteBuffer.allocate(Long.BYTES);
        buffer.put(bytes);
        buffer.flip();  //need flip
        return buffer.getLong();
    }

    public static long getMidnightTimestamp() {
        LocalDate today = LocalDate.now();
        LocalDateTime midnight = today.atStartOfDay();
        ZonedDateTime zonedMidnight = midnight.atZone(ZoneId.of("Asia/Shanghai"));
        return zonedMidnight.toInstant().getEpochSecond();
    }

    public SstRecordWriter(String fileName, String charSet, boolean ttlEnabled) throws IOException {
        this.isEmpty = true;
        this.charSet = charSet;
        Options options = new Options();
        options.setCreateIfMissing(true)
                .setWriteBufferSize(64 << 20)
                .setMaxWriteBufferNumber(4)
                .setTargetFileSizeBase(512 << 20);
        this.sstFileWriter = new SstFileWriter(new EnvOptions(), options);
        try {
            sstFileWriter.open(fileName);
        } catch (RocksDBException e) {
            throw new IOException(e);
        }
        this.midnightTS = longToBytes(getMidnightTimestamp());
        this.ttlEnabled = ttlEnabled;
    }

    public void write(String key, String value) throws IOException {
        byte[] keyBytes = key.getBytes(charSet);
        if (ttlEnabled) {
            keyBytes = concatByteArray(keyBytes, midnightTS);
        }
        try {
            sstFileWriter.put(keyBytes, value.getBytes(charSet));
        } catch (RocksDBException e) {
            ByteBuffer buffer = ByteBuffer.wrap(keyBytes);
            long tableId = buffer.getLong(0) >> 1;
            long hashId = buffer.getLong(8);
            throw new IOException(
                    "Write SST Error! TableId: [" + tableId + "], hashId: [" + hashId + "]", e);
        }
        this.isEmpty = false;
    }

    public boolean empty() {
        return isEmpty;
    }

    public void close() throws IOException {
        try {
            sstFileWriter.finish();
        } catch (RocksDBException e) {
            throw new IOException(e);
        }
    }

    private static byte[] concatByteArray(byte[] lhs, byte[] rhs) {
        byte[] result = new byte[lhs.length + rhs.length];
        System.arraycopy(lhs, 0, result, 0, lhs.length);
        System.arraycopy(rhs, 0, result, lhs.length, rhs.length);
        return result;
    }

    public static void main(String[] args) {
        byte[] x1 = longToBytes(getMidnightTimestamp());
        String x = "test";
        byte[] y = concatByteArray(x.getBytes(), x1);
        System.out.println(Arrays.toString(y));
    }
}
