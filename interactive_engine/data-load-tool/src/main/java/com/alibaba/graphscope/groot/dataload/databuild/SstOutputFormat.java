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

import com.alibaba.graphscope.groot.common.config.DataLoadConfig;
import org.apache.hadoop.conf.Configuration;
import org.apache.hadoop.fs.FileSystem;
import org.apache.hadoop.fs.Path;
import org.apache.hadoop.io.BytesWritable;
import org.apache.hadoop.mapreduce.RecordWriter;
import org.apache.hadoop.mapreduce.TaskAttemptContext;
import org.apache.hadoop.mapreduce.lib.output.FileOutputFormat;
import org.rocksdb.EnvOptions;
import org.rocksdb.Options;
import org.rocksdb.RocksDBException;
import org.rocksdb.SstFileWriter;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import java.io.IOException;
import java.nio.ByteBuffer;

public class SstOutputFormat extends FileOutputFormat<BytesWritable, BytesWritable> {

    private static final Logger logger = LoggerFactory.getLogger(SstOutputFormat.class);

    public static class SstRecordWriter extends RecordWriter<BytesWritable, BytesWritable> {

        private final SstFileWriter sstFileWriter;
        private final FileSystem fs;
        private final String fileName;
        private final Path path;
        private final boolean ttlEnabled;
        private final byte[] midnightTS;

        public SstRecordWriter(FileSystem fs, Path path, long ttlSec) throws RocksDBException {
            this.fs = fs;
            this.path = path;
            Options options = new Options();
            options.setCreateIfMissing(true)
                    .setWriteBufferSize(512 << 20)
                    .setMaxWriteBufferNumber(8)
                    .setTargetFileSizeBase(512 << 20);
            this.sstFileWriter = new SstFileWriter(new EnvOptions(), options);
            this.fileName = path.getName();
            sstFileWriter.open(fileName);
            this.midnightTS = Utils.longToBytes(Utils.getMidnightTimestamp());
            this.ttlEnabled = ttlSec > 0;
        }

        @Override
        public void write(BytesWritable key, BytesWritable value) throws IOException {
            byte[] keyBytes = key.copyBytes();
            if (ttlEnabled) {
                keyBytes = Utils.concatByteArray(keyBytes, midnightTS);
            }
            try {
                sstFileWriter.put(keyBytes, value.copyBytes());
            } catch (RocksDBException e) {
                ByteBuffer buffer = ByteBuffer.wrap(key.copyBytes());
                long tableId = buffer.getLong(0) >> 1;
                long hashId = buffer.getLong(8);
                throw new IOException(
                        "Write SST Error! TableId: [" + tableId + "], hashId: [" + hashId + "]", e);
            }
        }

        @Override
        public void close(TaskAttemptContext context) throws IOException {
            try {
                sstFileWriter.finish();
            } catch (RocksDBException e) {
                throw new IOException(e);
            }
            fs.copyFromLocalFile(true, new Path(fileName), path);
        }
    }

    @Override
    public RecordWriter<BytesWritable, BytesWritable> getRecordWriter(TaskAttemptContext job)
            throws IOException {
        Configuration conf = job.getConfiguration();
        String ttl = conf.get(DataLoadConfig.STORE_TTL_SEC);
        logger.info("ttl {}", ttl);
        long ttlSec = Long.parseLong(ttl);
        Path file = getDefaultWorkFile(job, ".sst");
        logger.info("output file [{}]", file);
        FileSystem fs = file.getFileSystem(conf);
        try {
            return new SstRecordWriter(fs, file, ttlSec);
        } catch (RocksDBException e) {
            throw new IOException(e);
        }
    }
}
