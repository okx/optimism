import csv
import matplotlib.pyplot as plt
import sys

def read_csv(filename):
    """Read CSV file and return data as lists."""
    x_data = []
    y_data = []
    try:
        with open(filename, 'r') as f:
            reader = csv.DictReader(f)
            for row in reader:
                # Convert keys to lowercase for case-insensitive matching
                row_lower = {k.lower(): v for k, v in row.items()}
                if 'block_number' in row_lower:
                    x_data.append(int(row_lower['block_number']))
                if 'tx_count' in row_lower:
                    y_data.append(int(row_lower['tx_count']))
                elif 'gas_used' in row_lower:
                    y_data.append(int(row_lower['gas_used']))
                elif 'base_fee' in row_lower:
                    y_data.append(int(row_lower['base_fee']))
        return x_data, y_data
    except FileNotFoundError:
        print(f"Error: File {filename} not found")
        return [], []
    except Exception as e:
        print(f"Error reading {filename}: {e}")
        return [], []

def read_histogram_csv(filename):
    """Read histogram CSV and return tx_counts and frequencies."""
    tx_counts = []
    frequencies = []
    try:
        with open(filename, 'r') as f:
            reader = csv.DictReader(f)
            for row in reader:
                row_lower = {k.lower(): v for k, v in row.items()}
                if 'tx_count' in row_lower and 'number_of_blocks' in row_lower:
                    tx_count = int(row_lower['tx_count'])
                    num_blocks = int(row_lower['number_of_blocks'])
                    # Only include non-zero frequencies for histogram
                    if num_blocks > 0:
                        tx_counts.append(tx_count)
                        frequencies.append(num_blocks)
        return tx_counts, frequencies
    except FileNotFoundError:
        print(f"Error: File {filename} not found")
        return [], []
    except Exception as e:
        print(f"Error reading {filename}: {e}")
        return [], []

def main():
    # Read data from CSV files
    print("Reading CSV files...")
    
    # Chart 1: Transaction count (line chart)
    block_numbers, tx_counts = read_csv("tx_count.csv")
    if not block_numbers:
        print("Warning: No data found in tx_count.csv")
    
    # Chart 2: Transaction histogram
    hist_tx_counts, hist_frequencies = read_histogram_csv("tx_histogram.csv")
    if not hist_tx_counts:
        print("Warning: No data found in tx_histogram.csv")
    
    # Chart 3: Gas used
    gas_block_numbers, gas_used_list = read_csv("gas_used.csv")
    if not gas_block_numbers:
        print("Warning: No data found in gas_used.csv")
    
    # Chart 4: Base fee
    base_fee_block_numbers, base_fees = read_csv("base_fee.csv")
    if not base_fee_block_numbers:
        print("Warning: No data found in base_fee.csv")
    
    # Create subplots - 4 plots: 3 line charts + 1 histogram
    fig, axes = plt.subplots(2, 2, figsize=(14, 10))
    
    # Plot 1: Transaction count (line chart)
    if block_numbers and tx_counts:
        axes[0, 0].plot(block_numbers, tx_counts, linewidth=1, color='blue')
        axes[0, 0].set_xlabel("Block number")
        axes[0, 0].set_ylabel("Transactions per block")
        axes[0, 0].set_title("Transactions per Block (Line Chart)")
        axes[0, 0].grid(True, alpha=0.3)
    else:
        axes[0, 0].text(0.5, 0.5, "No data available", ha='center', va='center')
        axes[0, 0].set_title("Transactions per Block (Line Chart)")
    
    # Plot 2: Transaction count (histogram)
    if hist_tx_counts and hist_frequencies:
        # Create histogram from frequency data
        # We need to expand the data: for each tx_count, repeat it 'frequency' times
        expanded_data = []
        for tx_count, freq in zip(hist_tx_counts, hist_frequencies):
            expanded_data.extend([tx_count] * freq)
        
        axes[0, 1].hist(expanded_data, bins=50, color='blue', alpha=0.7, edgecolor='black')
        axes[0, 1].set_xlabel("Transactions per block")
        axes[0, 1].set_ylabel("Frequency")
        axes[0, 1].set_title("Transaction Count Distribution (Histogram)")
        axes[0, 1].grid(True, alpha=0.3, axis='y')
    else:
        axes[0, 1].text(0.5, 0.5, "No data available", ha='center', va='center')
        axes[0, 1].set_title("Transaction Count Distribution (Histogram)")
    
    # Plot 3: Gas used
    if gas_block_numbers and gas_used_list:
        axes[1, 0].plot(gas_block_numbers, gas_used_list, linewidth=1, color='green')
        axes[1, 0].set_xlabel("Block number")
        axes[1, 0].set_ylabel("Gas used")
        axes[1, 0].set_title("Block Gas Used")
        axes[1, 0].grid(True, alpha=0.3)
    else:
        axes[1, 0].text(0.5, 0.5, "No data available", ha='center', va='center')
        axes[1, 0].set_title("Block Gas Used")
    
    # Plot 4: Base fee
    if base_fee_block_numbers and base_fees:
        axes[1, 1].plot(base_fee_block_numbers, base_fees, linewidth=1, color='red')
        axes[1, 1].set_xlabel("Block number")
        axes[1, 1].set_ylabel("Base fee (wei)")
        axes[1, 1].set_title("Block Base Fee")
        axes[1, 1].grid(True, alpha=0.3)
    else:
        axes[1, 1].text(0.5, 0.5, "No data available", ha='center', va='center')
        axes[1, 1].set_title("Block Base Fee")
    
    plt.tight_layout()
    plt.show()

if __name__ == "__main__":
    main()