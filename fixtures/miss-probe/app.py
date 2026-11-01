"""P1 recall probe: stdlib + third-party sinks from async def.

Expected findings for: smtp, ftp, zip, tar, boto get_object, glob/iterdir,
input, Future.result (plus already-covered copy/url/session/mongo/...).
"""

import smtplib, ftplib, shutil, zipfile, tarfile, sqlite3, json, os, glob, subprocess as sp, time as t
from pathlib import Path
from urllib.request import urlretrieve
import requests, boto3, pymongo, psycopg, httpx
from concurrent.futures import ThreadPoolExecutor
import pandas as pd

session = requests.Session()
s3 = boto3.client("s3")
mongo = pymongo.MongoClient()

async def h_smtp():
    smtplib.SMTP("localhost").sendmail("a", "b", "c")          # 13
async def h_ftp():
    ftplib.FTP("x").login()                                     # 15
async def h_copy():
    shutil.copy("a", "b")                                       # 17
async def h_zip():
    zipfile.ZipFile("a.zip").extractall()                       # 19
async def h_tar():
    tarfile.open("a.tar").extractall()                          # 21
async def h_urlretrieve():
    urlretrieve("http://x", "y")                                # 23
async def h_session():
    session.get("http://x")                                     # 25
async def h_boto():
    s3.get_object(Bucket="b", Key="k")                          # 27
async def h_mongo():
    mongo.db.c.find_one({})                                     # 29
async def h_psycopg():
    psycopg.connect("dbname=x")                                 # 31
async def h_alias():
    sp.run(["ls"])                                              # 33
    t.sleep(1)                                                  # 34
async def h_glob():
    glob.glob("*")                                              # 36
    list(Path(".").iterdir())                                   # 37
async def h_json():
    json.load(open("x.json"))                                   # 39
async def h_input():
    input("? ")                                                 # 41
async def h_pandas():
    pd.read_csv("x.csv")                                        # 43
async def h_future():
    ThreadPoolExecutor().submit(print).result()                 # 45
async def h_os():
    os.listdir("."); os.walk("."); os.system("ls")              # 47
async def h_httpx():
    httpx.get("http://x")                                       # 49
async def h_lambda():
    f = lambda: t.sleep(1)
    f()                                                         # 52
async def h_partial():
    import functools
    functools.partial(t.sleep, 1)()                             # 55
