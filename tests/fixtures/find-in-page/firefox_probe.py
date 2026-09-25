"""Record Firefox window.find behavior for basic.html (requires Selenium)."""

import json
import os
import time
from pathlib import Path

from selenium import webdriver
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.firefox.service import Service


def main():
    options = Options()
    options.add_argument("-headless")
    driver_path = os.environ.get("GECKODRIVER", "geckodriver")
    driver = webdriver.Firefox(service=Service(driver_path, log_output=os.devnull), options=options)
    fixture = Path(__file__).with_name("basic.html").resolve().as_uri()
    result = {"browser": driver.capabilities["browserName"],
              "version": driver.capabilities["browserVersion"], "cases": {},
              "boundaries": {}, "mutations": {}, "slot_order": [],
              "navigation": [], "navigation_reverse": []}
    try:
        for token, frames in (("ordinarytoken", False), ("autotoken", False),
                              ("hiddentoken", False), ("nonetoken", False),
                              ("shadowtoken", False), ("frametoken", True)):
            driver.get(fixture)
            found = driver.execute_script(
                "return window.find(arguments[0], false, false, false, false, arguments[1], false)",
                token, frames)
            time.sleep(0.2)  # Firefox completes the auto-content scroll asynchronously.
            state = driver.execute_script(
                "return {scrollY: window.scrollY, selection: String(getSelection()), "
                "frameSelection: String(document.getElementById('child').contentWindow.getSelection())}"
            )
            # Firefox window.find(searchInFrames=true) can report the same
            # child-frame selection again on every call, so it is not a count API.
            count = None if frames else int(found)
            if count is not None:
                for _ in range(20):
                    if not driver.execute_script(
                        "return window.find(arguments[0], false, false, false, false, false, false)",
                        token):
                        break
                    count += 1
                else:
                    raise RuntimeError(f"search did not terminate for {token}")
            result["cases"][token] = {"found": found, "count": count, **state}
        driver.get(fixture)
        for _ in range(3):
            result["navigation"].append(driver.execute_script(
                "const found=window.find('ordinarytoken');const s=getSelection();"
                "return {found:found,start:s.rangeCount?s.getRangeAt(0).startOffset:null}"))
        driver.get(fixture)
        for _ in range(2):
            driver.execute_script("window.find('ordinarytoken')")
        for _ in range(3):
            result["navigation_reverse"].append(driver.execute_script(
                "const found=window.find('ordinarytoken', false, true);const s=getSelection();"
                "return {found:found,start:s.rangeCount?s.getRangeAt(0).startOffset:null}"))
        for name in ("inline", "block"):
            driver.get(Path(__file__).with_name(name + ".html").resolve().as_uri())
            result["boundaries"][name] = {
                "found": driver.execute_script("return window.find('helloworld')"),
                "selection": driver.execute_script("return String(getSelection())"),
            }
        driver.get(fixture)
        result["mutations"]["before"] = driver.execute_script("return window.find('addedtoken')")
        result["mutations"]["after_add"] = driver.execute_script(
            "const p=document.createElement('p');p.id='added';p.textContent='addedtoken';"
            "document.body.append(p);return window.find('addedtoken')")
        result["mutations"]["after_remove"] = driver.execute_script(
            "document.getElementById('added').remove();getSelection().removeAllRanges();"
            "return window.find('addedtoken')")
        result["mutations"]["after_clear"] = driver.execute_script(
            "getSelection().removeAllRanges();return String(getSelection())")
        driver.get(Path(__file__).with_name("slot.html").resolve().as_uri())
        for _ in range(3):
            result["slot_order"].append(driver.execute_script(
                "const found=window.find('slotword');const s=getSelection();"
                "return {found:found,id:s.anchorNode?.parentElement?.id||''}"))
    finally:
        driver.quit()
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
